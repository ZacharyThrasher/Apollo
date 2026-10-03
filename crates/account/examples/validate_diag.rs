//! Diagnostic: why does the service accept or reject a jar? Prints only HTTP status, body size,
//! the logged_in flag and cookie-set statistics. Never prints values, names of identities, or
//! response bodies.
//!   cargo run -p apollo-account --example validate_diag -- firefox
//!   cargo run -p apollo-account --example validate_diag -- file:<netscape-file>

#[cfg(windows)]
fn main() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use apollo_account::identity::{
        AccountValidator, HttpPost, HttpResponse, InnerTubeValidator, NetError, UreqHttp,
    };
    use apollo_account::import;

    struct Spy {
        inner: UreqHttp,
        log: Mutex<Vec<String>>,
    }
    impl HttpPost for Spy {
        fn post(&self, url: &str, headers: &[(String, String)], body: &str) -> Result<HttpResponse, NetError> {
            let endpoint = url.split("/v1/").nth(1).unwrap_or("?").split('?').next().unwrap_or("?").to_string();
            let cookie_len = headers.iter().find(|(n, _)| n == "Cookie").map(|(_, v)| v.len()).unwrap_or(0);
            let auth_schemes: Vec<String> = headers
                .iter()
                .find(|(n, _)| n == "Authorization")
                .map(|(_, v)| v.split(' ').filter(|t| t.starts_with("SAPISID")).map(str::to_owned).collect())
                .unwrap_or_default();
            let r = self.inner.post(url, headers, body);
            let line = match &r {
                Ok(resp) => {
                    let j: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
                    let flag = j
                        .pointer("/responseContext/serviceTrackingParams")
                        .and_then(|v| v.as_array())
                        .and_then(|a| {
                            a.iter().find_map(|s| {
                                s["params"].as_array().and_then(|ps| {
                                    ps.iter().find(|p| p["key"] == "logged_in").map(|p| p["value"].to_string())
                                })
                            })
                        });
                    format!(
                        "{endpoint}: http={} body_bytes={} logged_in_flag={:?} cookie_header_bytes={cookie_len} auth_schemes={auth_schemes:?}",
                        resp.status,
                        resp.body.len(),
                        flag
                    )
                }
                Err(_) => format!("{endpoint}: transport error"),
            };
            self.log.lock().unwrap().push(line);
            r
        }
    }

    let arg = std::env::args().nth(1).unwrap_or_else(|| "firefox".into());
    let jar = if arg == "firefox" {
        let ff = PathBuf::from(std::env::var("APPDATA").unwrap()).join("Mozilla").join("Firefox");
        let profiles = import::firefox_profiles(&ff);
        let p = profiles.iter().find(|p| p.is_default).or(profiles.first()).expect("a profile");
        import::firefox_profile(&p.path).expect("firefox import")
    } else if let Some(f) = arg.strip_prefix("file:") {
        import::netscape_file(&PathBuf::from(f)).expect("file import")
    } else {
        panic!("usage: firefox | file:<path>");
    };

    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for c in &jar.cookies {
        *by_name.entry(c.name.as_str()).or_default() += 1;
    }
    let dupes: Vec<(&str, usize)> = by_name.iter().filter(|(_, n)| **n > 1).map(|(k, n)| (*k, *n)).collect();
    println!("cookies: {} (names with duplicates across domains: {})", jar.len(), dupes.len());
    for n in ["SAPISID", "__Secure-1PAPISID", "__Secure-3PAPISID", "SID", "__Secure-1PSID", "__Secure-3PSID", "LOGIN_INFO"] {
        let doms: Vec<&str> = jar.cookies.iter().filter(|c| c.name == n).map(|c| c.domain.as_str()).collect();
        println!("  {n}: domains {doms:?}");
    }
    let spy = Arc::new(Spy { inner: UreqHttp::default(), log: Mutex::new(vec![]) });
    let v = InnerTubeValidator::new(spy.clone());
    let out = v.validate(&jar);
    println!("verdict: {}", match out {
        apollo_account::identity::Validation::Valid { ref identities } => format!("Valid ({} identities)", identities.len()),
        ref other => format!("{other:?}"),
    });
    for l in spy.log.lock().unwrap().iter() {
        println!("  {l}");
    }
}

#[cfg(not(windows))]
fn main() {}
