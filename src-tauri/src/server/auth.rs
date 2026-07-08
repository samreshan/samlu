//! Token generation/verification for the local hook listener.
//!
//! Why a token at all, given the server only runs on 127.0.0.1: a webpage
//! can POST to a localhost port with `fetch(url, { mode: "no-cors" })`
//! without triggering a CORS preflight — the browser blocks it from reading
//! the *response*, but the request still lands. That's the same request
//! class behind several real localhost-dev-server CVEs. The token means a
//! request without the right path segment gets a 401 before we do anything
//! with it, regardless of where it came from.

use rand::Rng;

const TOKEN_LEN: usize = 32;

/// A random 32-character hex token, generated once at first run and stored
/// in the app's own config (see setup/mod.rs) — never derived from anything
/// guessable.
pub fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    (0..TOKEN_LEN)
        .map(|_| std::char::from_digit(rng.gen_range(0..16), 16).unwrap())
        .collect()
}

pub fn verify(expected: &str, candidate: &str) -> bool {
    // Constant-time-ish comparison isn't critical here (this isn't guarding
    // secrets with real value, just preventing notification spam/spoofing
    // from other local processes), but there's no reason not to compare the
    // full string rather than short-circuiting.
    expected.len() == candidate.len()
        && expected
            .bytes()
            .zip(candidate.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}
