use super::*;

/// The Host gate must be exactly as tight as its doc. The authority/port split
/// is pinned directly (brackets required for IPv6, unparseable ports refused),
/// and the gate accepts IP literals that are loopback — or any address on the
/// only non-loopback bind boot validation allows (`0.0.0.0`, the wildcard
/// bind) — plus `localhost`, each only on one of the harness's own configured
/// ports. Before the fix every IP literal passed, including a routable
/// test-net address and the wildcard bind, and `rsplit_once(':')` threw the
/// port away, so the routable-address and wrong-port legs read red against it;
/// the publish-bind accept leg read red against the dead equality-against-
/// `internal_host` check that shipped first.
#[test]
fn the_host_gate_accepts_loopback_published_binds_and_our_own_ports() {
    for (host, authority, port) in [
        ("127.0.0.1:8708", "127.0.0.1", Some(8708u16)),
        ("[::1]:8708", "::1", Some(8708)),
        ("[0:0:0:0:0:0:0:1]", "0:0:0:0:0:0:0:1", None),
        ("localhost", "localhost", None),
        ("localhost:8709", "localhost", Some(8709)),
    ] {
        assert_eq!(
            split_host(host),
            Some((authority, port)),
            "a Host a legitimate browser sends must parse: {host}"
        );
    }
    for host in [
        "::1",
        "127.0.0.1:",
        "127.0.0.1:99x",
        "127.0.0.1:8708:9",
        "[::1",
        "[::1]x",
    ] {
        assert_eq!(split_host(host), None, "malformed Host {host:?} is refused");
    }
    let default = ApiState::new(
        AppConfig::default(),
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );
    for host in [
        "127.0.0.1",
        "127.0.0.1:8708",
        "127.0.0.42:8711",
        "localhost:8708",
        "LOCALHOST",
        "[::1]:8710",
    ] {
        assert!(
            loopback_host(&default, host),
            "{host} addresses this harness"
        );
    }
    for host in [
        "203.0.113.9:8708",
        "0.0.0.0:8708",
        "10.1.2.3:8708",
        "198.51.100.7:8708",
        "evil.example.com",
        "::1",
        "[fe80::1]:8708",
        "127.0.0.1:9999",
        "[::1]:18109",
    ] {
        assert!(
            !loopback_host(&default, host),
            "{host} is not this harness's loopback address on its own port"
        );
    }
    // The publish posture is the only non-loopback bind boot `validate`
    // allows, so the second acceptance rule is pinned on `0.0.0.0`, never on
    // a hand-built unicast bind the boot validator refuses: any address the bind
    // covers answers on its own port, the port
    // rule still refuses a foreign port, and hostname clients stay refused
    // — the DNS-rebinding defense does not care how the harness is bound.
    let published = ApiState::new(
        AppConfig {
            internal_host: "0.0.0.0".parse().expect("publish bind"),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );
    assert!(
        loopback_host(&published, "192.168.1.23:8708"),
        "a published harness answers LAN clients addressing its real address"
    );
    assert!(
        !loopback_host(&published, "192.168.1.23:9999"),
        "the publish widening does not excuse a foreign port"
    );
    assert!(
        !loopback_host(&published, "evil.example:8708"),
        "a hostname Host stays refused even on the publish bind"
    );
}

/// The credential wall compares every byte of an
/// equal-length pair (`difference |= g ^ e`, never a short-circuiting `==`),
/// and the `Bearer` scheme is stripped in any ASCII case (RFC 7235) — the old
/// byte-exact `strip_prefix("Bearer ")` refused `bearer <token>` from
/// OpenAI-style clients with a message that read as a wrong token. The
/// structural legs read the shipped source, because a passing behavioral
/// matrix today does not stop `==` creeping back tomorrow.
#[test]
fn the_token_wall_is_constant_time_and_scheme_case_insensitive() {
    assert!(token_matches(b"secret", b"secret"));
    assert!(token_matches(b"", b""));
    for index in 0..6 {
        let mut differing = *b"secret";
        differing[index] ^= 0x20;
        assert!(
            !token_matches(&differing, b"secret"),
            "byte {index} must decide the verdict"
        );
    }
    assert!(!token_matches(b"secre", b"secret"), "shorter is refused");
    assert!(!token_matches(b"secre!", b"secret"), "longer is refused");
    for scheme in ["Bearer", "bearer", "BEARER", "BeArEr"] {
        assert_eq!(
            presented_token(&format!("{scheme} s3cret")),
            "s3cret",
            "RFC 7235: the auth-scheme is case-insensitive"
        );
    }
    assert_eq!(
        presented_token("s3cret"),
        "s3cret",
        "without a scheme the value IS the token"
    );
    assert_eq!(
        presented_token("bearer  s3cret "),
        "s3cret",
        "the trim matches `resolve_control_token`"
    );
    let source = include_str!("../guards.rs");
    let body_of = |name: &str| {
        let start = source
            .find(name)
            .unwrap_or_else(|| panic!("{name} must exist"));
        source[start..start + source[start..].find("\n}\n").expect("function end") + 3].to_string()
    };
    let auth = body_of("fn auth_reason(");
    assert!(
        auth.contains("token_matches(") && !auth.contains("given == expected"),
        "auth_reason must compare through the constant-time wall: back to `==` is \
             the FINDINGS-05 [8] regression: {auth}"
    );
    let fold = body_of("fn token_matches(");
    assert!(
        fold.contains("|=") && fold.contains('^'),
        "the wall must examine every byte of an equal-length pair: {fold}"
    );
    let scheme = body_of("fn presented_token(");
    assert!(
        scheme.contains("eq_ignore_ascii_case(\"bearer\")")
            && !scheme.contains("strip_prefix(\"Bearer \")"),
        "scheme stripping must be ASCII-case-insensitive: {scheme}"
    );
}

/// Pin (wiring half): the guard's only env read is
/// `crate::config::token_from_env` — the single `SIMPLEHARN_TOKEN` name is
/// pinned in `config`; a direct `std::env::var` here would drift from the
/// centralized reader while the guard stays green. Falsifier: revert this line
/// to a raw `std::env::var` and the second assert fires.
#[test]
fn the_control_token_reads_env_only_through_the_centralized_reader() {
    let source = include_str!("../guards.rs");
    let body = source
        .split("fn resolve_control_token(")
        .nth(1)
        .expect("the resolver must stay")
        .split("\n}")
        .next()
        .expect("the resolver must close");
    assert!(
        body.contains("crate::config::token_from_env()"),
        "the env read must go through the single centralized reader, not one raw name: {body}"
    );
    assert!(
        !body.contains("std::env::var"),
        "a second raw env read here bypasses the centralized reader: {body}"
    );
}

/// Pin (gated-route half): the old token name grants
/// nothing. The harness is configured with a file token while the legacy
/// variable carries a sentinel; the sentinel must NOT be the credential the
/// gate honours (401), and the file token must keep working beside it (200).
/// Falsifier: restore the old-name fallback in `token_from_env` — the sentinel
/// then resolves as the boot token, overriding the file value, and the first
/// assert goes red with a 200.
/// NAMING-CLEANUP-EXEMPT: the old spelling is set here only to prove the gate ignores it.
/// Safe to inject: no reader in this binary names the old spelling, so no other
/// test can observe the injection.
#[tokio::test]
async fn the_old_token_name_grants_nothing_to_a_gated_route() {
    const LEGACY: &str = "legacy-token-never-honored";
    const FILE_TOKEN: &str = "file-token-that-gates";
    // NAMING-CLEANUP-EXEMPT: the old token env spelling under test.
    std::env::set_var("SIMPLEHAR_TOKEN", LEGACY); // NAMING-CLEANUP-EXEMPT
    let guarded = Arc::new(ApiState::new(
        AppConfig {
            control_token: Some(FILE_TOKEN.to_string()),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));
    let presented = |token: &str| {
        let guarded = Arc::clone(&guarded);
        let token = token.to_string();
        async move {
            control_router(guarded)
                .oneshot(
                    Request::get("/health")
                        .header("host", "127.0.0.1:8708")
                        .header("x-har-token", token)
                        .body(Body::empty())
                        .expect("probe request"),
                )
                .await
                .expect("in-process probe")
                .status()
        }
    };
    let legacy_answer = presented(LEGACY).await;
    let file_answer = presented(FILE_TOKEN).await;
    std::env::remove_var("SIMPLEHAR_TOKEN"); // NAMING-CLEANUP-EXEMPT
    assert_eq!(
        legacy_answer,
        StatusCode::UNAUTHORIZED,
        "a request carrying the old name's value must be refused: it is not a credential"
    );
    assert_eq!(
        file_answer,
        StatusCode::OK,
        "the gate still stands on the file token beside the inert old name"
    );
}

/// The boot gate and `ApiState::new` must agree on whether a token resolves:
/// the refusal runs before the scheduler exists, so a divergence would refuse a
/// supported posture early or admit a refused one late. Falsifier: stop trimming
/// the file token and the whitespace-only leg fails; read the raw field in
/// `control_token_resolves` and the environment leg fails.
#[test]
fn the_boot_gate_reads_the_same_token_rules_as_the_guard() {
    let with_file = AppConfig {
        control_token: Some("  file-secret  ".to_string()),
        ..AppConfig::default()
    };
    assert!(
        control_token_resolves(&with_file),
        "a non-blank file token resolves"
    );
    let state = ApiState::new(
        with_file.clone(),
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        std::sync::Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );
    assert_eq!(
        state.requires_token(),
        control_token_resolves(&with_file),
        "the state and the boot gate must answer the same posture"
    );

    let blank = AppConfig {
        control_token: Some("   ".to_string()),
        ..AppConfig::default()
    };
    let env_resolves = crate::config::token_from_env().is_some_and(|raw| !raw.trim().is_empty());
    assert_eq!(
        control_token_resolves(&blank),
        env_resolves,
        "a whitespace-only file token must not resolve; the environment is the only other source"
    );
}
