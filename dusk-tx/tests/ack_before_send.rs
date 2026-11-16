//! `dusk-tx call --ack-before-send` as the agents run it: the RUES URL and
//! the key on stdin, which stays open, then the go-ahead once the
//! `Prepared TX` line is on stderr.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// How long one step may take before the test gives up on it.
const STEP: Duration = Duration::from_secs(30);

/// The transfer contract's answer to an `account` query: nonce 41.
fn account_reply() -> Vec<u8> {
    use rkyv::ser::{serializers::AllocSerializer, Serializer};
    let mut serializer = AllocSerializer::<256>::default();
    serializer
        .serialize_value(&dusk_core::transfer::moonlight::AccountData {
            nonce: 41,
            balance: 1_000_000_000_000_000,
        })
        .unwrap();
    serializer.into_serializer().into_inner().to_vec()
}

/// A RUES endpoint on localhost that reports chain ID 7, answers the
/// account query with nonce 41 and every other request with an empty 200,
/// one request a connection. Sends the path of each request it gets before
/// it answers.
fn mock_rues() -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (paths, received) = channel();
    let account = account_reply();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let path = first
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let reply: &[u8] = if path.ends_with("/chain_id") {
                &[7]
            } else if path.ends_with("/account") {
                &account
            } else {
                &[]
            };
            if paths.send(path).is_err() {
                return;
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            let _ = stream.write_all(reply);
        }
    });
    (url, received)
}

/// `call --ack-before-send`, started as the agents start it, with the URL
/// and key lines written and stdin left open.
struct Prepared {
    child: Child,
    stdin: ChildStdin,
    /// The hash on the `Prepared TX` line.
    tx_id: String,
    /// Stderr's lines after that one.
    stderr: Receiver<String>,
}

/// Start the call, signing with `nonce` or, without one, the account's
/// next, and wait for its `Prepared TX` line, which must come while stdin
/// is still open and nothing has answered it.
fn prepared(url: &str, nonce: Option<&str>) -> Prepared {
    let contract = "22".repeat(32);
    let mut child = Command::new(env!("CARGO_BIN_EXE_dusk-tx"))
        .args([
            "call",
            "--rues-url-stdin",
            "--expected-chain-id",
            "7",
            "--secret-key-stdin",
            "--contract",
            &contract,
            "--fn-name",
            "process",
            "--args",
            "0102",
            "--gas-limit",
            "5000000",
            "--gas-price",
            "1",
            "--no-wait",
            "--ack-before-send",
        ])
        .args(nonce.map(|nonce| ["--nonce", nonce]).into_iter().flatten())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    write!(stdin, "{url}\n{}\n", "11".repeat(32)).unwrap();
    stdin.flush().unwrap();
    let (lines, stderr) = channel();
    let pipe = child.stderr.take().unwrap();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { return };
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    let tx_id = loop {
        let line = stderr
            .recv_timeout(STEP)
            .expect("a Prepared TX line on stderr before the go-ahead");
        if let Some(rest) = line.trim_start().strip_prefix("Prepared TX ") {
            break rest.split(';').next().unwrap().to_owned();
        }
    };
    assert_eq!(tx_id.len(), 64, "{tx_id}");
    Prepared {
        child,
        stdin,
        tx_id,
        stderr,
    }
}

/// Wait for the child to exit, killing it if it takes longer than a step.
/// Returns its status and stdout.
fn finish(mut child: Child) -> (ExitStatus, String) {
    let deadline = Instant::now() + STEP;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("dusk-tx did not exit");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    (status, stdout)
}

/// With the nonce given, the `Prepared TX` line reaches stderr while
/// dusk-tx waits for the go-ahead, before anything but the chain ID query
/// reaches the node, and `send` then sends that transaction.
#[test]
fn the_prepared_line_is_out_before_the_go_ahead_and_send_sends_it() {
    let (url, requests) = mock_rues();
    // Stderr stays read to the end, so writing to it never fails.
    let Prepared {
        child,
        mut stdin,
        tx_id,
        stderr: _stderr,
    } = prepared(&url, Some("42"));
    let first = requests.recv_timeout(STEP).unwrap();
    assert!(first.ends_with("/chain_id"), "{first}");
    assert!(
        requests.recv_timeout(Duration::from_millis(500)).is_err(),
        "nothing more reaches the node before the go-ahead"
    );
    stdin.write_all(b"send\n").unwrap();
    drop(stdin);
    let (status, stdout) = finish(child);
    assert!(status.success(), "{stdout}");
    let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(output["success"], true);
    assert_eq!(output["tx_id"], tx_id);
    assert_eq!(output["submitted"], true);
    let sent: Vec<String> = requests.try_iter().collect();
    assert_eq!(
        sent,
        ["/on/transactions/preverify", "/on/transactions/propagate"]
    );
}

/// Closing stdin instead of sending the go-ahead sends nothing: dusk-tx
/// says so on stderr, and fails with an error that names the transaction
/// and says it wasn't submitted.
#[test]
fn closing_stdin_before_the_go_ahead_sends_nothing() {
    let (url, requests) = mock_rues();
    let Prepared {
        child,
        stdin,
        tx_id,
        stderr,
    } = prepared(&url, Some("42"));
    drop(stdin);
    let (status, stdout) = finish(child);
    assert_eq!(status.code(), Some(1), "{stdout}");
    let reason = "stdin closed before the line \"send\"";
    let said: Vec<String> = stderr
        .iter()
        .filter(|line| line.starts_with("Not submitted:"))
        .collect();
    assert_eq!(said, [format!("Not submitted: {reason}")]);
    let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(output["success"], false);
    assert_eq!(
        output["error"],
        format!("Transaction {tx_id} submission failed: Not submitted: {reason}")
    );
    let sent: Vec<String> = requests.try_iter().collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].ends_with("/chain_id"), "{sent:?}");
}

/// Without `--nonce`, dusk-tx asks the node for the signer's account, to
/// sign with its next nonce. Before the go-ahead the node gets those two
/// unsigned reads, the chain ID and the account, and nothing signed: no
/// preverify and no propagate. `send` then sends the transaction, at the
/// account's next nonce; closing stdin sends nothing more.
#[test]
fn without_a_nonce_only_unsigned_reads_come_before_the_go_ahead() {
    for go_ahead in [true, false] {
        let (url, requests) = mock_rues();
        let Prepared {
            child,
            mut stdin,
            tx_id,
            stderr: _stderr,
        } = prepared(&url, None);
        let before: Vec<String> = (0..2)
            .map(|_| requests.recv_timeout(STEP).unwrap())
            .collect();
        assert!(
            before[0].ends_with("/chain_id") && before[1].ends_with("/account"),
            "{before:?}"
        );
        assert!(
            requests.recv_timeout(Duration::from_millis(500)).is_err(),
            "nothing more reaches the node before the go-ahead"
        );
        if go_ahead {
            stdin.write_all(b"send\n").unwrap();
        }
        drop(stdin);
        let (status, stdout) = finish(child);
        let output: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let after: Vec<String> = requests.try_iter().collect();
        if go_ahead {
            assert!(status.success(), "{stdout}");
            assert_eq!(output["tx_id"], tx_id);
            assert_eq!(output["nonce"], 42, "the account's next nonce");
            assert_eq!(
                after,
                ["/on/transactions/preverify", "/on/transactions/propagate"]
            );
        } else {
            assert_eq!(status.code(), Some(1), "{stdout}");
            assert_eq!(after, Vec::<String>::new());
        }
    }
}
