//! Registry dependencies and `ryn publish` against an in-process mock of the
//! pods HTTP API, so the test needs neither the network nor the real registry.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ryn-registry-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temporary directory is created");
    dir
}

fn tar() -> Command {
    if cfg!(windows) {
        let system = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
            .join("System32")
            .join("tar.exe");
        if system.is_file() {
            return Command::new(system);
        }
    }
    Command::new("tar")
}

fn package_archive(root: &Path) -> Vec<u8> {
    let stage = root.join("stage");
    let package = stage.join("mathlib");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("ryn.yaml"),
        "name: mathlib\nversion: 1.2.0\nowner: tester\ndescription: Test math\ndependencies: {}\n",
    )
    .unwrap();
    fs::write(package.join("src").join("ops.ryn"), "pub fun triple(x: i32) -> i32 => x * 3\n").unwrap();
    let archive = root.join("mathlib.tar.gz");
    let status = tar()
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage)
        .arg("mathlib")
        .status()
        .expect("tar runs");
    assert!(status.success());
    fs::read(archive).unwrap()
}

#[derive(Default)]
struct Seen {
    uploads: Vec<(String, Vec<u8>)>,
}

/// Serves resolve, download and publish for one package until the test ends.
fn serve(archive: Vec<u8>, checksum: String, seen: Arc<Mutex<Seen>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let base = address.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                headers.push(line.trim().to_string());
            }
            let header = |name: &str| {
                headers.iter().find_map(|h| {
                    let (key, value) = h.split_once(':')?;
                    key.eq_ignore_ascii_case(name).then(|| value.trim().to_string())
                })
            };
            if header("Expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
                stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
            }
            let length: usize = header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").to_string();
            let (status, content_type, payload): (&str, &str, Vec<u8>) = if method == "GET"
                && path.starts_with("/api/v1/packages/mathlib/resolve")
            {
                (
                    "200 OK",
                    "application/json",
                    format!("{{\"version\": \"1.2.0\", \"checksum\": \"{checksum}\"}}").into_bytes(),
                )
            } else if method == "GET" && path == "/api/v1/packages/mathlib/1.2.0/download" {
                ("200 OK", "application/gzip", archive.clone())
            } else if method == "PUT" && path == "/api/v1/packages/new" {
                let auth = header("Authorization").unwrap_or_default();
                seen.lock().unwrap().uploads.push((auth, body.clone()));
                (
                    "200 OK",
                    "application/json",
                    format!("{{\"ok\": true, \"name\": \"usesmath\", \"version\": \"0.1.0\", \"url\": \"{base}/packages/usesmath/0.1.0\"}}").into_bytes(),
                )
            } else {
                (
                    "404 Not Found",
                    "application/json",
                    b"{\"errors\": [{\"detail\": \"not found\"}]}".to_vec(),
                )
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&payload);
        }
    });
    address
}

#[test]
fn registry_dependencies_lock_fetch_verify_and_publish() {
    let root = temp_dir("e2e");
    let archive = package_archive(&root);
    let checksum = ryn::registry::sha256_hex(&archive);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let registry = serve(archive, checksum.clone(), seen.clone());

    let app = root.join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("ryn.yaml"),
        "name: usesmath\nversion: 0.1.0\nowner: tester\ndescription: Uses mathlib\nlicense: MIT\nkeywords: [test]\ndependencies:\n  mathlib: ^1.0.0\n",
    )
    .unwrap();
    fs::write(
        app.join("src").join("main.ryn"),
        "use mathlib::ops\n\nfun main() -> i32 {\n    echo ops::triple(14)\n    return 0\n}\n",
    )
    .unwrap();

    let ryn = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ryn"))
            .args(args)
            .env("RYN_REGISTRY", &registry)
            .env("RYN_TOKEN", "pods_test_token")
            .output()
            .expect("ryn starts")
    };
    let app_arg = app.to_str().unwrap();

    // The first run creates ryn.lock, downloads, verifies and builds.
    let run = ryn(&["run", app_arg]);
    assert!(run.status.success(), "stderr: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
    let lock = fs::read_to_string(app.join("ryn.lock")).unwrap();
    assert!(lock.contains("version: 1.2.0"), "{lock}");
    assert!(lock.contains(&format!("checksum: {checksum}")), "{lock}");
    assert!(lock.contains(&format!("source: registry:{registry}")), "{lock}");

    // A tampered pin is refused before anything is unpacked.
    fs::remove_dir_all(app.join(".ryn")).unwrap();
    fs::write(app.join("ryn.lock"), lock.replace(&checksum, &"0".repeat(64))).unwrap();
    let tampered = ryn(&["run", app_arg]);
    assert!(!tampered.status.success());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("checksum mismatch"));
    fs::write(app.join("ryn.lock"), &lock).unwrap();

    // Publishing is rejected for path dependencies and sends the token otherwise.
    let publish = ryn(&["publish", app_arg]);
    assert!(publish.status.success(), "stderr: {}", String::from_utf8_lossy(&publish.stderr));
    assert!(String::from_utf8_lossy(&publish.stdout).contains("/packages/usesmath/0.1.0"));
    let uploads = &seen.lock().unwrap().uploads;
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].0, "Bearer pods_test_token");
    assert_eq!(&uploads[0].1[..2], &[0x1f, 0x8b], "upload is gzip");

    fs::write(
        app.join("ryn.yaml"),
        "name: usesmath\nversion: 0.1.1\nowner: tester\ndependencies:\n  local:\n    path: ../local\n",
    )
    .unwrap();
    let rejected = ryn(&["publish", app_arg]);
    assert!(!rejected.status.success());
    let _ = fs::remove_dir_all(&root);
}
