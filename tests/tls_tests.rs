use rcgen::{CertificateParams, KeyPair};
use sbx::TlsConfig;
use tempfile::TempDir;

fn write_cert_and_key(dir: &TempDir) -> (String, String) {
    let key_pair = KeyPair::generate().unwrap();
    let params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let cert = params.self_signed(&key_pair).unwrap();

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();

    (
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
}

#[test]
fn test_load_valid_cert_and_key() {
    let dir = TempDir::new().unwrap();
    let (cert_path, key_path) = write_cert_and_key(&dir);

    let server_config = TlsConfig::new(cert_path, key_path).load().unwrap();
    assert_eq!(
        server_config.alpn_protocols,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    );
}

#[test]
fn test_load_missing_cert_file() {
    let dir = TempDir::new().unwrap();
    let cert_path = dir.path().join("nope.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, KeyPair::generate().unwrap().serialize_pem()).unwrap();

    let err = TlsConfig::new(
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
    .load()
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn test_load_missing_key_file() {
    let dir = TempDir::new().unwrap();
    let key_pair = KeyPair::generate().unwrap();
    let params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.self_signed(&key_pair).unwrap();

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("nope.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();

    let err = TlsConfig::new(
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
    .load()
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn test_load_empty_cert_file() {
    let dir = TempDir::new().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, "").unwrap();
    std::fs::write(&key_path, KeyPair::generate().unwrap().serialize_pem()).unwrap();

    let err = TlsConfig::new(
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
    .load()
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no certificates found"));
}

#[test]
fn test_load_invalid_cert_pem() {
    let dir = TempDir::new().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, "not a PEM file").unwrap();
    std::fs::write(&key_path, KeyPair::generate().unwrap().serialize_pem()).unwrap();

    let err = TlsConfig::new(
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
    .load()
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn test_load_empty_key_file() {
    let dir = TempDir::new().unwrap();
    let key_pair = KeyPair::generate().unwrap();
    let params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.self_signed(&key_pair).unwrap();

    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, "").unwrap();

    let err = TlsConfig::new(
        cert_path.to_string_lossy().to_string(),
        key_path.to_string_lossy().to_string(),
    )
    .load()
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no private key found"));
}
