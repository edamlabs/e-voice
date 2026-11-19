use e_voice_core::schema::lang::Lang;
use e_voice_daemon::speech::notice::Notice;
use serde_json::json;

#[test]
fn test_each_known_notice_has_its_exact_phrase_in_each_language() {
    let phrases = [
        (Notice::Permission, Lang::Es, "Necesito tu permiso."),
        (Notice::Permission, Lang::En, "I need your permission."),
        (Notice::Waiting, Lang::Es, "Estoy esperando."),
        (Notice::Waiting, Lang::En, "I'm waiting for you."),
    ];
    for (notice, lang, phrase) in phrases {
        assert_eq!(notice.text(lang), phrase, "{notice:?} {lang:?}");
    }
}

#[test]
fn test_only_the_two_wire_words_parse_as_a_notice() {
    let parse = |value: serde_json::Value| serde_json::from_value::<Notice>(value).ok();
    assert_eq!(parse(json!("permission_prompt")), Some(Notice::Permission));
    assert_eq!(parse(json!("idle_prompt")), Some(Notice::Waiting));
    for other in [
        json!("auth_success"),
        json!("agent_needs_input"),
        json!("elicitation_dialog"),
        json!("Permission_Prompt"),
        json!(" permission_prompt"),
        json!(""),
        json!(null),
        json!(3),
        json!(["permission_prompt"]),
        json!({"permission_prompt": 1}),
    ] {
        assert_eq!(parse(other.clone()), None, "{other}");
    }
}
