//! Adversarial test for `stream_to_log` (review item #1/#8): a process that writes
//! far more than a pipe buffer to stderr and exits nonzero must not deadlock, and
//! its stderr must be captured into the merged log (failure diagnostics).

use std::io::Read;

use sia::orchestrator::stream_to_log;

#[test]
fn test_stream_to_log_captures_heavy_stderr_and_exit_code() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("out.log");

    // 50k lines (~288 KiB) to stderr — well over the ~64 KiB pipe buffer. If stderr
    // weren't drained concurrently the child would block and this test would hang.
    let script = "seq 1 50000 1>&2; echo DONE_STDOUT; exit 3";
    let code = stream_to_log(
        &["sh".to_string(), "-c".to_string(), script.to_string()],
        log.to_str().unwrap(),
    )
    .expect("stream_to_log should not error");

    assert_eq!(code, 3, "exit code must propagate");

    let mut contents = String::new();
    std::fs::File::open(&log)
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert!(contents.contains("DONE_STDOUT"), "stdout must be captured");
    assert!(
        contents.contains("\n50000\n") || contents.contains("50000"),
        "heavy stderr must be captured (no deadlock, no diagnostic loss)"
    );
}
