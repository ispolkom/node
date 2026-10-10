package com.yandi.yandi_mobile;

import java.security.MessageDigest;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import javax.net.ssl.X509TrustManager;

/** A pinned socket must never become a trust-all socket when its pin is missing. */
public final class PinnedTrustManager implements X509TrustManager {
    private final byte[] expected;

    public PinnedTrustManager(String fingerprint) throws CertificateException {
        String pin = fingerprint == null ? "" : fingerprint.trim();
        if (!pin.matches("[0-9a-fA-F]{64}")) {
            throw new CertificateException("Valid TLS SHA-256 fingerprint is required; pair again");
        }
        expected = new byte[32];
        for (int i = 0; i < expected.length; i++) {
            expected[i] = (byte) Integer.parseInt(pin.substring(i * 2, i * 2 + 2), 16);
        }
    }

    @Override public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
        if (chain == null || chain.length == 0) throw new CertificateException("Missing TLS certificate");
        try {
            byte[] actual = MessageDigest.getInstance("SHA-256").digest(chain[0].getEncoded());
            if (!MessageDigest.isEqual(expected, actual)) throw new CertificateException("TLS fingerprint mismatch");
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new CertificateException(e);
        }
    }

    @Override public void checkClientTrusted(X509Certificate[] chain, String authType) throws CertificateException {
        throw new CertificateException("Client certificates are not supported");
    }
    @Override public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
}
