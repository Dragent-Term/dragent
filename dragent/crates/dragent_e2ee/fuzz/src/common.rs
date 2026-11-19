//! Shared driver: seeded mutational fuzzing with no third-party harness.
//!
//! Each `src/bin/fuzz_*.rs` target supplies a `check` function over arbitrary
//! bytes: decoders must never panic, and every successful decode must
//! re-encode byte-exactly. The same entry points are what CI (E11) wires to
//! cargo-fuzz; here they run on stable with a xorshift mutator and a corpus
//! seeded from `vectors/codec/*.json`.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Not-copyable PRNG state (xorshift64*; splitmix-seeded).
pub struct Rng(u64);

impl Rng {
    /// Seed from an explicit value (reproduces a run).
    pub fn seeded(seed: u64) -> Self {
        let mut s = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        s = (s ^ (s >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Rng(s ^ (s >> 31))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform value below `n` (`n > 0`; modulo bias is irrelevant here).
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        (self.next() as usize) % n
    }

    /// A random byte.
    pub fn byte(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
}

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("vectors").join("codec")
}

/// Pull every `"bytes_hex": "<hex>"` value out of the vector files.
pub fn corpus_hex(filenames: &[&str]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for name in filenames {
        let text = std::fs::read_to_string(vectors_dir().join(name)).expect("read vectors");
        let mut rest = text.as_str();
        while let Some(i) = rest.find("\"bytes_hex\"") {
            rest = &rest[i + 11..];
            let start = rest.find('"').expect("hex value") + 1;
            let end = rest[start..].find('"').expect("hex end") + start;
            let hex = &rest[start..end];
            assert!(hex.len().is_multiple_of(2));
            out.push(
                (0..hex.len())
                    .step_by(2)
                    .map(|j| u8::from_str_radix(&hex[j..j + 2], 16).expect("hex"))
                    .collect(),
            );
            rest = &rest[end..];
        }
    }
    out
}

/// Pull every `"json_utf8"` / `"content_utf8"` string value out, unescaping
/// JSON string syntax minimally (the generator writes real UTF-8, ASCII
/// escapes only for quotes and backslashes).
pub fn corpus_str(filenames: &[&str], keys: &[&str]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for name in filenames {
        let text = std::fs::read_to_string(vectors_dir().join(name)).expect("read vectors");
        for key in keys {
            let mut rest = text.as_str();
            let pat = format!("\"{key}\"");
            while let Some(i) = rest.find(&pat) {
                rest = &rest[i + pat.len()..];
                let start = rest.find('"').expect("str value") + 1;
                let mut s = String::new();
                let mut esc = false;
                let mut idx = start;
                let bytes = rest.as_bytes();
                loop {
                    let c = bytes[idx];
                    if esc {
                        match c {
                            b'n' => s.push('\n'),
                            b't' => s.push('\t'),
                            b'r' => s.push('\r'),
                            b'"' => s.push('"'),
                            b'\\' => s.push('\\'),
                            b'/' => s.push('/'),
                            b'u' => {
                                let h = std::str::from_utf8(&bytes[idx + 1..idx + 5])
                                    .expect("unicode escape");
                                let cp = u32::from_str_radix(h, 16).expect("hex");
                                s.push(char::from_u32(cp).expect("scalar"));
                                idx += 4;
                            }
                            x => panic!("unexpected escape {x}"),
                        }
                        esc = false;
                    } else if c == b'\\' {
                        esc = true;
                    } else if c == b'"' {
                        break;
                    } else {
                        s.push(c as char);
                    }
                    idx += 1;
                }
                out.push(s.into_bytes());
                rest = &rest[idx..];
            }
        }
    }
    out
}

/// Mutate `buf` in place, drawing splice material from `corpus`.
/// `max_len` caps the output so huge inputs do not starve the run.
pub fn mutate(rng: &mut Rng, corpus: &[Vec<u8>], buf: &mut Vec<u8>, max_len: usize) {
    if buf.is_empty() || rng.below(4) == 0 {
        // Fresh base: a corpus entry or random bytes.
        if !corpus.is_empty() && rng.below(3) > 0 {
            *buf = corpus[rng.below(corpus.len())].clone();
        } else {
            let n = 1 + rng.below(max_len.min(512));
            buf.resize(n, 0);
            for b in buf.iter_mut() {
                *b = rng.byte();
            }
            return;
        }
    }
    for _ in 0..1 + rng.below(4) {
        match rng.below(8) {
            0 => {
                // Flip bytes.
                for _ in 0..1 + rng.below(8) {
                    if buf.is_empty() {
                        break;
                    }
                    let i = rng.below(buf.len());
                    buf[i] ^= 1 << rng.below(8);
                }
            }
            1 => {
                // Set bytes.
                for _ in 0..1 + rng.below(8) {
                    if buf.is_empty() {
                        break;
                    }
                    let i = rng.below(buf.len());
                    let v = rng.byte();
                    buf[i] = v;
                }
            }
            2 => {
                // Insert a run.
                if buf.len() < max_len {
                    let i = rng.below(buf.len() + 1);
                    let n = 1 + rng.below((max_len - buf.len()).min(32));
                    let mut ins = vec![0u8; n];
                    for b in ins.iter_mut() {
                        *b = rng.byte();
                    }
                    buf.splice(i..i, ins);
                }
            }
            3 => {
                // Delete a range.
                if buf.len() > 1 {
                    let i = rng.below(buf.len());
                    let n = 1 + rng.below((buf.len() - i).min(32));
                    buf.drain(i..i + n);
                }
            }
            4 => {
                // Duplicate a chunk elsewhere.
                if !buf.is_empty() && buf.len() < max_len {
                    let i = rng.below(buf.len());
                    let n = 1 + rng.below((buf.len() - i).min(32));
                    let chunk = buf[i..i + n].to_vec();
                    let at = rng.below(buf.len() + 1);
                    let take = chunk.len().min(max_len - buf.len());
                    buf.splice(at..at, chunk[..take].to_vec());
                }
            }
            5 => {
                // Splice in a slice of another corpus entry.
                if !corpus.is_empty() && !buf.is_empty() {
                    let other = &corpus[rng.below(corpus.len())];
                    if !other.is_empty() && buf.len() < max_len {
                        let o = rng.below(other.len());
                        let n = 1 + rng.below((other.len() - o).min(32));
                        let at = rng.below(buf.len() + 1);
                        let take = n.min(max_len - buf.len());
                        buf.splice(at..at, other[o..o + take].to_vec());
                    }
                }
            }
            6 => {
                // Truncate.
                if buf.len() > 1 {
                    buf.truncate(rng.below(buf.len()));
                }
            }
            _ => {
                // Random fill of a subrange (structured-prefix breaker).
                if !buf.is_empty() {
                    let i = rng.below(buf.len());
                    let n = 1 + rng.below((buf.len() - i).min(64));
                    for b in &mut buf[i..i + n] {
                        *b = rng.byte();
                    }
                }
            }
        }
    }
    buf.truncate(max_len);
}

/// Usage: `fuzz_<target> [seconds=600] [seed=random] [extra-corpus-dir]`.
/// Also loads checked-in `fuzz/corpus/<target>/` seeds when present. Runs
/// `check` on mutated inputs until the deadline. A `panic!` or an `Err` saves
/// the input to `crashes/` (with the seed in the report) and exits 1; a clean
/// run exits 0.
pub fn run(
    target: &str,
    corpus: Vec<Vec<u8>>,
    seeds: Vec<Vec<u8>>,
    max_len: usize,
    check: impl Fn(&[u8]) -> Result<(), String>,
) -> ! {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600);
    let seed: u64 = args
        .get(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x1234_5678)
                ^ ((std::process::id() as u64) << 32)
        });
    println!("{target}: seed={seed} seconds={secs}");
    let mut rng = Rng::seeded(seed);
    let mut extra = Vec::new();
    let checked_in = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus").join(target);
    if checked_in.is_dir() {
        for entry in std::fs::read_dir(&checked_in).expect("read checked-in corpus") {
            let entry = entry.expect("corpus entry");
            if entry.file_type().expect("type").is_file() {
                extra.push(std::fs::read(entry.path()).expect("read seed"));
            }
        }
        println!("{target}: {} checked-in seeds", extra.len());
    }
    if let Some(dir) = args.get(3) {
        for entry in std::fs::read_dir(dir).expect("read extra corpus") {
            let entry = entry.expect("corpus entry");
            if entry.file_type().expect("type").is_file() {
                extra.push(std::fs::read(entry.path()).expect("read seed"));
            }
        }
    }
    let mut full_corpus = corpus;
    full_corpus.extend(seeds);
    full_corpus.extend(extra);
    assert!(!full_corpus.is_empty(), "empty corpus");
    // Warm up on the seeds themselves first (they must all pass).
    for (i, seed_input) in full_corpus.clone().iter().enumerate() {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(seed_input)));
        match r {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => crash(target, seed, seed_input, &format!("seed {i} violates property: {msg}")),
            Err(_) => crash(target, seed, seed_input, &format!("seed {i} panics")),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut buf: Vec<u8> = Vec::new();
    let mut execs: u64 = 0;
    let mut last_report = Instant::now();
    while Instant::now() < deadline {
        for _ in 0..1000 {
            mutate(&mut rng, &full_corpus, &mut buf, max_len);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&buf)));
            match r {
                Ok(Ok(())) => {}
                Ok(Err(msg)) => crash(target, seed, &buf, &format!("property: {msg}")),
                Err(_) => crash(target, seed, &buf, &"panic"),
            }
            execs += 1;
            if Instant::now() >= deadline {
                break;
            }
        }
        if last_report.elapsed() >= Duration::from_secs(60) {
            eprintln!("{target}: {execs} execs so far, clean");
            last_report = Instant::now();
        }
    }
    println!("{target}: clean: {execs} execs in {secs}s");
    std::process::exit(0)
}

fn crash(target: &str, seed: u64, input: &[u8], why: &str) -> ! {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crashes");
    std::fs::create_dir_all(&dir).expect("crash dir");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{target}-{stamp}-{}.bin", input.len()));
    std::fs::write(&path, input).expect("write crash");
    eprintln!("{target}: FAILURE seed={seed} ({why}); input saved to {}", path.display());
    std::process::exit(1)
}
