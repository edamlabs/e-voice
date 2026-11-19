use e_voice_core::schema::lang::Lang;
use e_voice_daemon::talk::prompt::TalkPrompt;

#[test]
fn test_the_voice_prompt_is_pinned_in_each_language() {
    assert_eq!(
        TalkPrompt::Voice.text(Lang::Es),
        "Te están escuchando, no leyendo: contesta en frases cortas pensadas para decirse en voz alta, sin markdown, sin código y sin listas. Antes de una respuesta larga, pregunta si la quieren. Di qué has hecho, no cómo."
    );
    assert_eq!(
        TalkPrompt::Voice.text(Lang::En),
        "You are being heard, not read: answer in short sentences meant to be spoken, with no markdown, code or lists. Before a long answer, ask whether it is wanted. Say what you did, not how."
    );
}

#[test]
fn test_each_language_has_its_own_prompt_and_no_prompt_is_empty_or_multiline() {
    let prompts: Vec<&str> = Lang::ALL.iter().map(|lang| TalkPrompt::Voice.text(*lang)).collect();
    assert_eq!(prompts.len(), 2);
    assert_ne!(prompts[0], prompts[1]);
    for prompt in prompts {
        assert!(!prompt.trim().is_empty() && !prompt.contains('\n') && prompt.ends_with('.'));
        assert_eq!(prompt, prompt.trim());
        assert!(prompt.chars().count() < 400, "{}", prompt.chars().count());
    }
}
