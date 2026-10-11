//! Linux local UI transport. Authentication is a connection capability, never a header.
//! The router and its StepUp/vault handlers are shared with TCP.
use axum::{
    body::Body,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Router,
};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
};

const NAME: &str = "yandi-ui.sock";
const CONNECTIONS: usize = 32;
const BODY_LIMIT: usize = 201 * 1024 * 1024; // preserves the existing group upload route
const IDLE: Duration = Duration::from_secs(60);
const REQUEST: Duration = Duration::from_secs(300);

/// Only this module can construct the capability, after peer_cred succeeds.
#[derive(Clone, Copy)]
pub(super) struct LocalOwner(());

pub(super) fn is_local(req: &axum::extract::Request) -> bool {
    req.extensions().get::<LocalOwner>().is_some()
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unsafe UI socket directory or entry",
    )
}
fn uid_matches(peer: u32, owner: u32) -> bool {
    peer == owner
}
fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| denied())
}
fn stat_at(dir: &File, name: &str) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::uninit();
    let name = cstr(name)?;
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            st.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { st.assume_init() })
}
fn unlink(dir: &File, name: &str) -> io::Result<()> {
    let name = cstr(name)?;
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn socket_live(path: &Path) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { File::from_raw_fd(fd) };
    if unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&addr) as libc::socklen_t,
        )
    } == 0
    {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::ConnectionRefused {
        Ok(false)
    } else if e.kind() == io::ErrorKind::WouldBlock || e.raw_os_error() == Some(libc::EINPROGRESS) {
        Ok(true)
    } else {
        Err(e)
    }
}

/// Walk using dirfds: no symlink in any component. Ancestors must be trusted,
/// the data directory and run directory private. Never repair unsafe permissions.
fn private_dir(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(denied());
    }
    let parts: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();
    if parts.is_empty() {
        return Err(denied());
    }
    let owner = unsafe { libc::geteuid() };
    let mut dir = File::open("/")?;
    for (i, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(denied());
        };
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(name.as_bytes()).map_err(|_| denied())?;
        let last = i + 1 == parts.len();
        let mut fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
            if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0
                && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
            {
                return Err(io::Error::last_os_error());
            }
            fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
        }
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let next = unsafe { File::from_raw_fd(fd) };
        let m = next.metadata()?;
        let mode = m.mode();
        let trusted_sticky = m.uid() == 0 && mode & libc::S_ISVTX != 0;
        if last {
            if m.uid() != owner || mode & 0o077 != 0 || mode & 0o700 != 0o700 {
                return Err(denied());
            }
        } else if (m.uid() != owner && m.uid() != 0) || (mode & 0o022 != 0 && !trusted_sticky) {
            return Err(denied());
        }
        dir = next;
    }
    Ok(dir)
}

pub(super) struct Endpoint {
    listener: UnixListener,
    dir: File,
    _lock: File,
    inode: u64,
    device: u64,
}
impl Endpoint {
    pub(super) fn bind(data_dir: &Path) -> io::Result<Self> {
        // Fail before creating run or socket if the parent is unsafe.
        let data = private_dir(data_dir)?;
        let run = cstr("run")?;
        if unsafe { libc::mkdirat(data.as_raw_fd(), run.as_ptr(), 0o700) } != 0
            && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
        {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe {
            libc::openat(
                data.as_raw_fd(),
                run.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let dir = unsafe { File::from_raw_fd(fd) };
        let m = dir.metadata()?;
        let owner = unsafe { libc::geteuid() };
        if m.uid() != owner || m.mode() & 0o777 != 0o700 {
            return Err(denied());
        }
        // Anchored bind/chmod/open even if an ancestor is renamed concurrently.
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(anchored.join(".ui.lock"))?;
        let lm = lock.metadata()?;
        if !lm.is_file() || lm.uid() != owner || lm.nlink() != 1 || lm.mode() & 0o777 != 0o600 {
            return Err(denied());
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "UI socket listener already running",
            ));
        }
        match stat_at(&dir, NAME) {
            Ok(st) => {
                if st.st_mode & libc::S_IFMT != libc::S_IFSOCK
                    || st.st_uid != owner
                    || st.st_mode & 0o777 != 0o600
                    || st.st_nlink != 1
                {
                    return Err(denied());
                }
                match socket_live(&anchored.join(NAME)) {
                    Ok(true) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "UI socket already listening",
                        ))
                    }
                    Ok(false) => unlink(&dir, NAME)?,
                    Err(e) => return Err(e),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(anchored.join(NAME))?;
        let name = cstr(NAME)?;
        if unsafe { libc::fchmodat(dir.as_raw_fd(), name.as_ptr(), 0o600, 0) } != 0 {
            let e = io::Error::last_os_error();
            let _ = unlink(&dir, NAME);
            return Err(e);
        }
        let st = stat_at(&dir, NAME)?;
        Ok(Self {
            listener,
            dir,
            _lock: lock,
            inode: st.st_ino,
            device: st.st_dev,
        })
    }

    pub(super) async fn serve(self, app: Router) -> io::Result<()> {
        let permits = Arc::new(Semaphore::new(CONNECTIONS));
        loop {
            let (stream, _) = self.listener.accept().await?;
            // SO_PEERCRED, before constructing HTTP parser or reading a byte.
            let cred = match stream.peer_cred() {
                Ok(c) => c,
                Err(_) => continue,
            };
            if !uid_matches(cred.uid(), unsafe { libc::geteuid() }) {
                continue;
            }
            let permit = match permits.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let app = app
                .clone()
                .layer(axum::Extension(LocalOwner(())))
                .layer(axum::middleware::from_fn(socket_request));
            tokio::spawn(async move {
                // The permit lives in IO, including the upgraded websocket, not just HTTP.
                let io = hyper_util::rt::TokioIo::new(BoundedIo {
                    stream,
                    _permit: permit,
                    read_timer: Box::pin(tokio::time::sleep(IDLE)),
                    write_timer: Box::pin(tokio::time::sleep(IDLE)),
                    write_waiting: false,
                });
                let service = hyper_util::service::TowerToHyperService::new(app);
                let mut builder = hyper::server::conn::http1::Builder::new();
                builder
                    .keep_alive(false)
                    .max_headers(64)
                    .max_buf_size(32 * 1024)
                    .timer(hyper_util::rt::TokioTimer::new())
                    .header_read_timeout(Duration::from_secs(15));
                let _ = tokio::time::timeout(
                    REQUEST,
                    builder.serve_connection(io, service).with_upgrades(),
                )
                .await;
                // No URI, request body, credentials, or parser error in logs.
            });
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Ok(st) = stat_at(&self.dir, NAME) {
            if st.st_ino == self.inode && st.st_dev == self.device {
                let _ = unlink(&self.dir, NAME);
            }
        }
    }
}

async fn socket_request(mut req: axum::extract::Request, next: Next) -> Response {
    if !req.uri().path().starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Hard transport ceiling also applies to handlers that don't extract Json/Bytes.
    // Existing per-route DefaultBodyLimit (usually 2MiB) is still enforced.
    if req
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|n| n > BODY_LIMIT as u64)
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let body = std::mem::replace(req.body_mut(), Body::empty());
    *req.body_mut() = Body::new(http_body_util::Limited::new(body, BODY_LIMIT));
    match tokio::time::timeout(REQUEST, next.run(req)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

struct BoundedIo {
    stream: UnixStream,
    _permit: tokio::sync::OwnedSemaphorePermit,
    read_timer: std::pin::Pin<Box<tokio::time::Sleep>>,
    write_timer: std::pin::Pin<Box<tokio::time::Sleep>>,
    write_waiting: bool,
}
impl AsyncRead for BoundedIo {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        use std::{future::Future, task::Poll};
        let before = buf.filled().len();
        match std::pin::Pin::new(&mut self.stream).poll_read(cx, buf) {
            Poll::Ready(r) => {
                if buf.filled().len() > before {
                    self.read_timer
                        .as_mut()
                        .reset(tokio::time::Instant::now() + IDLE);
                }
                Poll::Ready(r)
            }
            Poll::Pending => {
                if self.read_timer.as_mut().poll(cx).is_ready() {
                    Poll::Ready(Err(io::ErrorKind::TimedOut.into()))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}
impl AsyncWrite for BoundedIo {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        use std::{future::Future, task::Poll};
        match std::pin::Pin::new(&mut self.stream).poll_write(cx, buf) {
            Poll::Ready(r) => {
                self.write_waiting = false;
                if r.as_ref().is_ok_and(|n| *n > 0) {
                    self.write_timer
                        .as_mut()
                        .reset(tokio::time::Instant::now() + IDLE);
                }
                Poll::Ready(r)
            }
            Poll::Pending => {
                if !self.write_waiting {
                    self.write_waiting = true;
                    self.write_timer
                        .as_mut()
                        .reset(tokio::time::Instant::now() + IDLE);
                }
                if self.write_timer.as_mut().poll(cx).is_ready() {
                    Poll::Ready(Err(io::ErrorKind::TimedOut.into()))
                } else {
                    Poll::Pending
                }
            }
        }
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn foreign_uid_is_rejected_and_owner_is_accepted() {
        let owner = unsafe { libc::geteuid() };
        assert!(uid_matches(owner, owner));
        assert!(!uid_matches(owner.wrapping_add(1), owner));
        assert!(!uid_matches(0, 1000));
    }

    #[tokio::test]
    async fn private_modes_stale_recovery_and_exclusive_listener() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ep = Endpoint::bind(&data).unwrap();
        let socket = data.join("run").join(NAME);
        for (path, mode) in [(&data, 0o700), (&data.join("run"), 0o700), (&socket, 0o600)] {
            assert_eq!(
                std::fs::symlink_metadata(path).unwrap().mode() & 0o777,
                mode
            );
        }
        assert!(
            Endpoint::bind(&data).is_err(),
            "second listener must not remove the first socket"
        );
        assert!(socket.exists());
        drop(ep);
        assert!(!socket.exists());
        let stale = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(stale);
        let recovered = Endpoint::bind(&data).unwrap();
        drop(recovered);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn unsafe_parents_run_symlinks_and_socket_entries_are_never_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Endpoint::bind(&data).is_err());
        assert!(
            !data.join("run").exists(),
            "must fail before creating a socket or run directory"
        );
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = data.join("run");
        std::fs::create_dir(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Endpoint::bind(&data).is_err());
        assert!(!run.join(NAME).exists());
        std::fs::remove_dir(&run).unwrap();
        std::os::unix::fs::symlink(dir.path(), &run).unwrap();
        assert!(Endpoint::bind(&data).is_err());
        std::fs::remove_file(&run).unwrap();
        std::fs::create_dir(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = run.join(NAME);
        std::fs::write(&socket, b"do not delete").unwrap();
        assert!(Endpoint::bind(&data).is_err());
        assert_eq!(std::fs::read(&socket).unwrap(), b"do not delete");
        std::fs::remove_file(&socket).unwrap();
        std::os::unix::fs::symlink(dir.path().join("target"), &socket).unwrap();
        assert!(Endpoint::bind(&data).is_err());
        assert!(std::fs::symlink_metadata(&socket).unwrap().is_symlink());
    }

    #[tokio::test]
    async fn unsafe_socket_modes_and_symlinked_data_parent_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ep = Endpoint::bind(&data).unwrap();
        let socket = data.join("run").join(NAME);
        drop(ep);
        let stale = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        drop(stale);
        assert!(Endpoint::bind(&data).is_err());
        assert_eq!(
            std::fs::symlink_metadata(&socket).unwrap().mode() & 0o777,
            0o666
        );
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&data, &alias).unwrap();
        assert!(Endpoint::bind(&alias.join("new-data")).is_err());
        assert!(!data.join("new-data").exists());
    }

    #[tokio::test]
    async fn socket_limits_content_length_and_hides_web_pages() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ep = Endpoint::bind(&data).unwrap();
        let app = Router::new()
            .route("/api/echo", axum::routing::post(|| async { "ok" }))
            .route("/", axum::routing::get(|| async { "web" }));
        let task = tokio::spawn(ep.serve(app));
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (request, expected) in [
            (
                "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
                "404",
            ),
            (
                format!(
                    "POST /api/echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
                    BODY_LIMIT + 1
                ),
                "413",
            ),
        ] {
            let mut stream = UnixStream::connect(data.join("run").join(NAME))
                .await
                .unwrap();
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut b = [0u8; 512];
            let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut b))
                .await
                .unwrap()
                .unwrap();
            assert!(String::from_utf8_lossy(&b[..n]).starts_with(&format!("HTTP/1.1 {expected}")));
        }
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn connection_budget_refuses_excess_clients_before_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let ep = Endpoint::bind(&data).unwrap();
        let task = tokio::spawn(ep.serve(Router::new()));
        let mut held = Vec::new();
        for _ in 0..CONNECTIONS {
            let mut socket = UnixStream::connect(data.join("run").join(NAME))
                .await
                .unwrap();
            // Incomplete headers keep each permitted connection alive.
            socket
                .write_all(b"GET /api/status HTTP/1.1\r\n")
                .await
                .unwrap();
            held.push(socket);
        }
        let mut excess = UnixStream::connect(data.join("run").join(NAME))
            .await
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), excess.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        drop(held);
        task.abort();
        let _ = task.await;
    }
}
