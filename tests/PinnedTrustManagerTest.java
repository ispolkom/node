import com.yandi.yandi_mobile.PinnedTrustManager;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.security.MessageDigest;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

public class PinnedTrustManagerTest {
    interface Checked { void run() throws Exception; }
    static void refuses(Checked action) throws Exception {
        try { action.run(); } catch (CertificateException expected) { return; }
        throw new AssertionError("Untrusted certificate/pin accepted");
    }
    public static void main(String[] args) throws Exception {
        refuses(() -> new PinnedTrustManager(""));
        refuses(() -> new PinnedTrustManager(" "));
        refuses(() -> new PinnedTrustManager("not-a-fingerprint"));
        X509Certificate cert;
        try (java.io.InputStream in = Files.newInputStream(Paths.get(args[0]))) {
            cert = (X509Certificate) CertificateFactory.getInstance("X.509").generateCertificate(in);
        }
        StringBuilder pin = new StringBuilder();
        for (byte b : MessageDigest.getInstance("SHA-256").digest(cert.getEncoded())) pin.append(String.format("%02x", b));
        PinnedTrustManager trust = new PinnedTrustManager(pin.toString().toUpperCase());
        trust.checkServerTrusted(new X509Certificate[]{cert}, "EC");
        refuses(() -> trust.checkServerTrusted(new X509Certificate[0], "EC"));
        refuses(() -> new PinnedTrustManager("00".repeat(32)).checkServerTrusted(new X509Certificate[]{cert}, "EC"));
        System.out.println("TLS pin tests passed");
    }
}
