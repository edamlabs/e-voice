use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fake::{Fake, closed_port, ecli, ecli_with};

fn only(fake: &Fake) -> crate::fake::Seen {
    let seen = fake.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    seen[0].clone()
}

fn said(fake: &Fake, args: &[&str]) -> crate::fake::Out {
    let url = fake.url();
    let mut all = args.to_vec();
    all.extend(["--url", &url]);
    ecli(&all)
}

fn refused(out: &crate::fake::Out, line: &str) {
    assert_eq!(out.code, Some(1), "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, format!("ecli: {line}\n"));
}

#[test]
fn test_say_posts_the_text_with_null_voice_and_lang_as_json() {
    let fake = Fake::json(200, r#"{"id":0,"outcome":"done"}"#);
    said(&fake, &["say", "hola mundo"]);
    let seen = only(&fake);
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/speak"));
    assert_eq!(seen.content_type.as_deref(), Some("application/json"));
    assert_eq!(seen.json(), json!({"text": "hola mundo", "voice": null, "lang": null}));
    assert_eq!(seen.json().as_object().unwrap().len(), 3);
}

#[test]
fn test_say_carries_voice_and_lang_untouched_even_when_the_daemon_will_refuse_them() {
    let fake = Fake::json(200, r#"{"id":0,"outcome":"done"}"#);
    said(
        &fake,
        &[
            "say",
            "¿Qué tal? \"citado\"\nlínea",
            "--voice",
            "ef_dora",
            "--lang",
            "fr",
        ],
    );
    assert_eq!(
        only(&fake).json(),
        json!({"text": "¿Qué tal? \"citado\"\nlínea", "voice": "ef_dora", "lang": "fr"})
    );
}

#[test]
fn test_say_takes_a_text_that_looks_like_a_flag_after_the_separator() {
    let fake = Fake::json(200, r#"{"id":0,"outcome":"done"}"#);
    let url = fake.url();
    let out = ecli(&["say", "--url", &url, "--", "--no-es-flag"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(only(&fake).json()["text"], "--no-es-flag");
}

#[test]
fn test_stop_posts_an_empty_object() {
    let fake = Fake::json(200, r#"{"stopped":null,"dropped":0}"#);
    said(&fake, &["stop"]);
    let seen = only(&fake);
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/stop"));
    assert_eq!(seen.content_type.as_deref(), Some("application/json"));
    assert_eq!(seen.body, "{}");
}

#[test]
fn test_listen_posts_a_null_timeout_or_the_one_given_verbatim() {
    let fake = Fake::json(200, r#"{"text":"x","lang":"es","emotion":{}}"#);
    said(&fake, &["listen"]);
    said(&fake, &["listen", "--timeout", "8s"]);
    said(&fake, &["listen", "--timeout", "soon", "--struct"]);
    let seen = fake.requests();
    assert!(
        seen.iter()
            .all(|r| (r.method.as_str(), r.path.as_str()) == ("POST", "/listen"))
    );
    assert!(
        seen.iter()
            .all(|r| r.content_type.as_deref() == Some("application/json"))
    );
    let bodies: Vec<Value> = seen.iter().map(crate::fake::Seen::json).collect();
    assert_eq!(
        bodies,
        [
            json!({"timeout": null}),
            json!({"timeout": "8s"}),
            json!({"timeout": "soon"})
        ]
    );
}

#[test]
fn test_say_done_and_cancelled_both_exit_0_with_the_outcome_and_the_id() {
    for (body, line) in [
        (r#"{"id":3,"outcome":"done"}"#, "done 3\n"),
        (r#"{"id":4,"outcome":"cancelled"}"#, "cancelled 4\n"),
        (
            r#"{"id":18446744073709551615,"outcome":"done"}"#,
            "done 18446744073709551615\n",
        ),
    ] {
        let out = said(&Fake::json(200, body), &["say", "x"]);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(0), line, ""),
            "{body}"
        );
    }
}

#[test]
fn test_say_failures_print_the_daemons_error_and_exit_1() {
    let cases = [
        (
            400,
            r#"{"error":"text is empty","outcome":"failed"}"#,
            "daemon answered 400: text is empty",
        ),
        (
            400,
            r#"{"error":"text is too long: 5000 characters, at most 4096","outcome":"failed"}"#,
            "daemon answered 400: text is too long: 5000 characters, at most 4096",
        ),
        (
            429,
            r#"{"error":"speech queue is full: 16 requests already waiting","outcome":"failed"}"#,
            "daemon answered 429: speech queue is full: 16 requests already waiting",
        ),
        (
            502,
            r#"{"error":"speech service: cut","outcome":"failed"}"#,
            "daemon answered 502: speech service: cut",
        ),
        (
            503,
            r#"{"error":"no output device","outcome":"failed"}"#,
            "daemon answered 503: no output device",
        ),
        (
            500,
            r#"{"error":"speech queue stopped","outcome":"failed"}"#,
            "daemon answered 500: speech queue stopped",
        ),
    ];
    for (status, body, line) in cases {
        refused(&said(&Fake::json(status, body), &["say", "x"]), line);
    }
}

#[test]
fn test_stop_prints_what_it_stopped_or_that_nothing_played() {
    let busy = said(&Fake::json(200, r#"{"stopped":3,"dropped":2}"#), &["stop"]);
    assert_eq!(
        (busy.code, busy.stdout.as_str(), busy.stderr.as_str()),
        (Some(0), "stopped 3, dropped 2\n", "")
    );
    let zero = said(&Fake::json(200, r#"{"stopped":0,"dropped":0}"#), &["stop"]);
    assert_eq!(zero.stdout, "stopped 0, dropped 0\n");
    let idle = said(&Fake::json(200, r#"{"stopped":null,"dropped":0}"#), &["stop"]);
    assert_eq!(
        (idle.code, idle.stdout.as_str(), idle.stderr.as_str()),
        (Some(0), "nothing playing\n", "")
    );
    refused(
        &said(&Fake::json(500, r#"{"error":"speech queue stopped"}"#), &["stop"]),
        "daemon answered 500: speech queue stopped",
    );
}

const HEARD: &str =
    r#"{"text":"hola, ¿qué tal?","lang":"es","emotion":{"label":"happy","scores":{"happy":0.75},"model":"ser"}}"#;

#[test]
fn test_listen_prints_the_text_alone() {
    let out = said(&Fake::json(200, HEARD), &["listen"]);
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (Some(0), "hola, ¿qué tal?\n", "")
    );
    let multi = said(
        &Fake::json(200, r#"{"text":"uno\ndos","lang":"en","emotion":{}}"#),
        &["listen"],
    );
    assert_eq!(multi.stdout, "uno\ndos\n");
}

#[test]
fn test_listen_struct_is_the_daemons_answer_as_one_compact_json_line() {
    let out = said(&Fake::json(200, HEARD), &["listen", "--struct"]);
    assert_eq!((out.code, out.stderr.as_str()), (Some(0), ""));
    assert_eq!(out.stdout.lines().count(), 1, "{:?}", out.stdout);
    assert!(out.stdout.ends_with('\n') && !out.stdout.ends_with("\n\n"));
    let line = out.stdout.trim_end_matches('\n');
    let body: Value = serde_json::from_str(HEARD).unwrap();
    assert_eq!(serde_json::from_str::<Value>(line).unwrap(), body);
    assert_eq!(line, body.to_string(), "not compact");
    let pretty = said(
        &Fake::json(
            200,
            "{\n  \"text\": \"a\\nb\",\n  \"lang\": \"es\",\n  \"emotion\": {}\n}",
        ),
        &["listen", "--struct"],
    );
    assert_eq!(pretty.stdout.lines().count(), 1, "{:?}", pretty.stdout);
}

#[test]
fn test_listen_failures_print_the_daemons_error_and_exit_1() {
    let cases = [
        (
            400,
            r#"{"error":"a listen timeout is above zero and at most 300 s, not 0 s"}"#,
            "daemon answered 400: a listen timeout is above zero and at most 300 s, not 0 s",
        ),
        (
            408,
            r#"{"error":"nobody started speaking within 8 s"}"#,
            "daemon answered 408: nobody started speaking within 8 s",
        ),
        (
            409,
            r#"{"error":"someone is already listening"}"#,
            "daemon answered 409: someone is already listening",
        ),
        (
            502,
            r#"{"error":"speech recognizer: no final within 40 s of speech"}"#,
            "daemon answered 502: speech recognizer: no final within 40 s of speech",
        ),
        (
            503,
            r#"{"error":"no input device"}"#,
            "daemon answered 503: no input device",
        ),
    ];
    for (status, body, line) in cases {
        for flags in [&["listen"][..], &["listen", "--struct"][..]] {
            refused(&said(&Fake::json(status, body), flags), line);
        }
    }
}

#[test]
fn test_a_refusal_that_is_not_the_daemons_shape_prints_the_raw_body() {
    let text = "Failed to deserialize the JSON body into the target type: lang: unknown variant `fr`, expected `es` or `en` at line 1 column 40";
    let raw = Fake::answering(422, "text/plain; charset=utf-8", text);
    refused(
        &said(&raw, &["say", "x", "--lang", "fr"]),
        &format!("daemon answered 422: {text}"),
    );
    for (body, shown) in [
        (r#"{"detail":"nope"}"#, r#"{"detail":"nope"}"#),
        (r#"{"error":5}"#, r#"{"error":5}"#),
        (r#"{"error":null}"#, r#"{"error":null}"#),
        ("", ""),
        ("<html>bad gateway</html>", "<html>bad gateway</html>"),
    ] {
        refused(
            &said(&Fake::json(500, body), &["stop"]),
            &format!("daemon answered 500: {shown}"),
        );
    }
}

#[test]
fn test_a_success_in_a_shape_the_verb_cannot_read_is_an_unreadable_answer() {
    let cases: [(&[&str], &'static str); 7] = [
        (&["say", "x"], r#"{"id":"x","outcome":"done"}"#),
        (&["say", "x"], r#"{"outcome":"done"}"#),
        (&["say", "x"], ""),
        (&["say", "x"], "ok"),
        (&["stop"], r#"{"stopped":3}"#),
        (&["listen"], r#"{"lang":"es"}"#),
        (&["listen"], r#"{"text":7}"#),
    ];
    for (args, body) in cases {
        let out = said(&Fake::json(200, body), args);
        assert_eq!(out.code, Some(1), "{args:?} {body}");
        assert_eq!(out.stdout, "");
        let shown = serde_json::from_str::<Value>(body)
            .ok()
            .filter(|_| args == ["listen"])
            .map_or_else(|| body.to_owned(), |v| v.to_string());
        assert_eq!(
            out.stderr,
            format!("ecli: unreadable answer from the daemon: {shown}\n"),
            "{args:?} {body}"
        );
    }
}

#[test]
fn test_a_204_or_any_2xx_is_read_like_a_200() {
    let ok = said(&Fake::json(201, r#"{"id":1,"outcome":"done"}"#), &["say", "x"]);
    assert_eq!((ok.code, ok.stdout.as_str()), (Some(0), "done 1\n"));
    let empty = said(&Fake::answering(204, "application/json", ""), &["stop"]);
    assert_eq!(empty.code, Some(1));
    assert_eq!(empty.stderr, "ecli: unreadable answer from the daemon: \n");
}

#[test]
fn test_a_daemon_that_is_not_there_is_a_cannot_reach_error_naming_the_url() {
    let url = format!("http://127.0.0.1:{}", closed_port());
    for args in [&["say", "x"][..], &["stop"][..], &["listen"][..]] {
        let mut all = args.to_vec();
        all.extend(["--url", &url]);
        let started = Instant::now();
        let out = ecli(&all);
        assert_eq!(out.code, Some(1), "{args:?}");
        assert_eq!(out.stdout, "");
        let prefix = format!("ecli: cannot reach the daemon at {url}: ");
        assert!(out.stderr.starts_with(&prefix), "{}", out.stderr);
        assert!(
            out.stderr.len() > prefix.len() + 1 && out.stderr.ends_with('\n'),
            "{:?}",
            out.stderr
        );
        assert_eq!(out.stderr.lines().count(), 1, "{:?}", out.stderr);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }
}

#[test]
fn test_a_daemon_that_hangs_up_or_a_url_that_is_not_one_is_a_cannot_reach_error() {
    let hung = Fake::hanging_up();
    let out = said(&hung, &["stop"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stderr
            .starts_with(&format!("ecli: cannot reach the daemon at {}: ", hung.url())),
        "{}",
        out.stderr
    );
    let bad = ecli(&["stop", "--url", "not a url"]);
    assert_eq!(bad.code, Some(1));
    assert!(
        bad.stderr.starts_with("ecli: cannot reach the daemon at not a url: "),
        "{}",
        bad.stderr
    );
}

#[test]
fn test_there_is_no_request_timeout_a_slow_daemon_is_waited_for() {
    let slow = Fake::delayed(
        Duration::from_secs(7),
        200,
        "application/json",
        r#"{"id":9,"outcome":"done"}"#,
    );
    let started = Instant::now();
    let out = said(&slow, &["say", "largo"]);
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (Some(0), "done 9\n", "")
    );
    assert!(started.elapsed() >= Duration::from_secs(7));
}

#[test]
fn test_the_environment_url_is_used_and_the_flag_overrides_it() {
    let env = Fake::json(200, r#"{"stopped":null,"dropped":0}"#);
    let flag = Fake::json(200, r#"{"stopped":7,"dropped":1}"#);
    let by_env = ecli_with(&["stop"], Some(&env.url()));
    assert_eq!(by_env.stdout, "nothing playing\n");
    assert_eq!((env.requests().len(), flag.requests().len()), (1, 0));
    let by_flag = ecli_with(&["stop", "--url", &flag.url()], Some(&env.url()));
    assert_eq!(by_flag.stdout, "stopped 7, dropped 1\n");
    assert_eq!((env.requests().len(), flag.requests().len()), (1, 1));
    let dead = format!("http://127.0.0.1:{}", closed_port());
    assert_eq!(ecli_with(&["stop", "--url", &flag.url()], Some(&dead)).code, Some(0));
    let unset = ecli_with(&["stop"], Some(&dead));
    assert_eq!(unset.code, Some(1));
    assert!(unset.stderr.contains(&dead), "{}", unset.stderr);
}

#[test]
fn test_trailing_slashes_in_the_url_do_not_reach_the_path() {
    for suffix in ["/", "//", "///"] {
        let fake = Fake::json(200, r#"{"id":0,"outcome":"done"}"#);
        let url = format!("{}{suffix}", fake.url());
        let out = ecli(&["say", "x", "--url", &url]);
        assert_eq!(out.code, Some(0), "{suffix}: {}", out.stderr);
        assert_eq!(only(&fake).path, "/speak", "{suffix}");
    }
}

#[test]
fn test_every_verb_works_through_the_environment_url_with_a_trailing_slash() {
    let fake = Fake::json(200, r#"{"text":"ok","lang":"es","emotion":{}}"#);
    let url = format!("{}/", fake.url());
    assert_eq!(ecli_with(&["listen"], Some(&url)).stdout, "ok\n");
    assert_eq!(only(&fake).path, "/listen");
}

#[test]
fn test_an_answer_cut_short_is_an_unreadable_answer_not_a_hang() {
    let started = Instant::now();
    for args in [&["say", "x"][..], &["stop"][..], &["listen"][..]] {
        let out = said(&Fake::truncating(), args);
        assert_eq!(out.code, Some(1), "{args:?}");
        assert_eq!(out.stdout, "");
        assert!(
            out.stderr.starts_with("ecli: unreadable answer from the daemon: "),
            "{}",
            out.stderr
        );
        assert_eq!(out.stderr.lines().count(), 1, "{:?}", out.stderr);
    }
    assert!(started.elapsed() < Duration::from_secs(5));
}
