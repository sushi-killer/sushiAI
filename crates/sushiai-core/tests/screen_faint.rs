use sushiai_core::Screen;

/// Walks an escape-sequence stream and reports whether the first character of
/// `needle` is drawn while faint (SGR 2) is active.
fn faint_at(stream: &[u8], needle: &str) -> Option<bool> {
    let mut faint = false;
    let mut text = String::new();
    let mut states: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < stream.len() {
        if stream[i] == 0x1b && stream.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < stream.len() && !(0x40..=0x7e).contains(&stream[j]) {
                j += 1;
            }
            if j >= stream.len() {
                break;
            }
            if stream[j] == b'm' {
                let body = String::from_utf8_lossy(&stream[i + 2..j]).to_string();
                let params: Vec<&str> = if body.is_empty() {
                    vec!["0"]
                } else {
                    body.split(';').collect()
                };
                let mut k = 0;
                while k < params.len() {
                    match params[k] {
                        "" | "0" => faint = false,
                        "2" => faint = true,
                        "22" => faint = false,
                        "38" | "48" => {
                            k += if params.get(k + 1) == Some(&"5") {
                                2
                            } else {
                                4
                            };
                        }
                        _ => {}
                    }
                    k += 1;
                }
            }
            i = j + 1;
            continue;
        }
        if stream[i] >= 0x20 && stream[i] < 0x7f {
            text.push(stream[i] as char);
            states.push(faint);
        }
        i += 1;
    }
    text.find(needle).map(|at| states[at])
}

#[test]
fn snapshot_keeps_faint_text() {
    let mut screen = Screen::new(24, 80);
    screen.feed(b"> \x1b[2mrun the tests\x1b[0m");
    let snapshot = screen.snapshot();
    assert_eq!(faint_at(&snapshot, "run the tests"), Some(true));
}

#[test]
fn faint_does_not_leak_past_reset() {
    let mut screen = Screen::new(24, 80);
    screen.feed(b"\x1b[2mghost\x1b[22m typed\r\n\x1b[2mmore\x1b[0m plain");
    let snapshot = screen.snapshot();
    assert_eq!(faint_at(&snapshot, "ghost"), Some(true));
    assert_eq!(faint_at(&snapshot, "typed"), Some(false));
    assert_eq!(faint_at(&snapshot, "plain"), Some(false));
}
