//! Страницы веб-интерфейса не подменяют `window.confirm`: вызывающий код ждёт синхронный ответ, а подменённое асинхронное окно сразу
//! «разрешало» действие (удаление устройства, отзыв доверия и т. п.) ещё до нажатия «Нет».
use std::path::Path;

#[test]
fn no_page_overrides_window_confirm() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/ui");
    let mut checked = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "html" || x == "js") {
            let text = std::fs::read_to_string(&p).unwrap();
            assert!(
                !(text.contains("window.confirm") && text.contains("window.confirm =") || text.contains("window.confirm=")),
                "{} подменяет window.confirm",
                p.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 3, "страницы интерфейса не найдены");
}
