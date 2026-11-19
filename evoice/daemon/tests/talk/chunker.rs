use e_voice_daemon::talk::chunker::SentenceChunker;
use e_voice_daemon::workflow::queue::TEXT;

/// Every piece the chunker says for `deltas`, push by push, and what `finish` adds.
fn run(deltas: &[&str]) -> (Vec<Vec<String>>, Vec<String>) {
    let mut chunker = SentenceChunker::default();
    let pushed = deltas.iter().map(|delta| chunker.push(delta)).collect();
    (pushed, chunker.finish())
}

fn all(deltas: &[&str]) -> Vec<String> {
    let (pushed, finished) = run(deltas);
    pushed.into_iter().flatten().chain(finished).collect()
}

fn owned(pieces: &[&str]) -> Vec<String> {
    pieces.iter().map(|piece| (*piece).to_owned()).collect()
}

#[test]
fn test_a_sentence_is_said_on_the_delta_that_proves_it_closed() {
    let (pushed, finished) = run(&["Hola. ¿Qué", " tal?", " Bien."]);
    assert_eq!(pushed[0], owned(&["Hola."]));
    assert_eq!(
        pushed[1],
        Vec::<String>::new(),
        "a mark ending a line still arriving can continue (v3. -> v3.11)"
    );
    assert_eq!(pushed[2], owned(&["¿Qué tal?"]));
    assert_eq!(finished, owned(&["Bien."]));
}

#[test]
fn test_a_last_sentence_held_for_its_mark_comes_out_at_finish() {
    let (pushed, finished) = run(&["Hola. ¿Qué", " tal?"]);
    assert_eq!(pushed[0], owned(&["Hola."]));
    assert_eq!(pushed[1], Vec::<String>::new());
    assert_eq!(finished, owned(&["¿Qué tal?"]));
    assert_eq!(all(&["… tal?"]), owned(&["tal?"]));
}

#[test]
fn test_the_last_word_of_a_line_still_arriving_is_not_final() {
    let (pushed, finished) = run(&["Hola mundo.", " Adiós"]);
    assert_eq!(pushed[0], Vec::<String>::new(), "no whitespace after the mark yet");
    assert_eq!(
        pushed[1],
        owned(&["Hola mundo."]),
        "the next word proves the first sentence closed"
    );
    assert_eq!(finished, owned(&["Adiós"]));
}

#[test]
fn test_finish_flushes_an_open_sentence_with_no_mark_and_not_before() {
    let (pushed, finished) = run(&["Hola mundo sin punto"]);
    assert_eq!(pushed, vec![Vec::<String>::new()]);
    assert_eq!(finished, owned(&["Hola mundo sin punto"]));
    let (pushed, finished) = run(&["Uno. Dos. Tres sin"]);
    assert_eq!(pushed[0], owned(&["Uno.", "Dos."]));
    assert_eq!(finished, owned(&["Tres sin"]));
}

#[test]
fn test_the_end_of_a_line_closes_a_sentence_without_a_mark() {
    let (pushed, finished) = run(&["Título sin punto\n", "Otra línea.\n"]);
    assert_eq!(pushed[0], owned(&["Título sin punto"]));
    assert_eq!(pushed[1], owned(&["Otra línea."]));
    assert_eq!(finished, Vec::<String>::new());
}

#[test]
fn test_crlf_lines_are_read_like_lf_lines() {
    assert_eq!(all(&["Hola.\r\nAdiós.\r\n"]), owned(&["Hola.", "Adiós."]));
}

#[test]
fn test_a_version_number_is_never_split_whatever_the_delta_boundaries() {
    for deltas in [
        vec!["v3.", "11.0 listo."],
        vec!["v", "3", ".", "1", "1", ".0 ", "listo", "."],
        vec!["v3.11.0 listo."],
    ] {
        assert_eq!(all(&deltas), owned(&["v3.11.0 listo."]), "{deltas:?}");
    }
}

#[test]
fn test_abbreviations_do_not_close_a_sentence() {
    assert_eq!(
        all(&["Use e.g. this. And that. "]),
        owned(&["Use e.g. this.", "And that."])
    );
    assert_eq!(
        all(&["Por ejemplo, p. ej. esto. Y eso. "]),
        owned(&["Por ejemplo, p. ej. esto.", "Y eso."])
    );
}

#[test]
fn test_markdown_is_reduced_to_what_reads_aloud() {
    assert_eq!(all(&["# Título\nTexto aquí.\n"]), owned(&["Texto aquí."]));
    assert_eq!(all(&["- uno\n- dos\n"]), owned(&["uno", "dos"]));
    assert_eq!(
        all(&["Mira `queue.rs` ahora. Y [la doc](https://x.y/z) también. "]),
        owned(&["Mira queue.rs ahora.", "Y la doc también."])
    );
    assert_eq!(all(&["Hecho 🎉 ✅ listo. "]), owned(&["Hecho listo."]));
    assert_eq!(all(&["...\n---\n"]), Vec::<String>::new());
}

#[test]
fn test_a_fenced_block_is_never_said_whatever_the_boundaries() {
    let reply = "Antes.\n```rust\nSECRETO fn x() {}\n```\nDespués.\n";
    assert_eq!(all(&[reply]), owned(&["Antes.", "Después."]));
    let chars: Vec<String> = reply.chars().map(String::from).collect();
    let deltas: Vec<&str> = chars.iter().map(String::as_str).collect();
    assert_eq!(all(&deltas), owned(&["Antes.", "Después."]));
}

#[test]
fn test_a_fence_arriving_one_backtick_at_a_time_is_held_then_hidden() {
    let (pushed, finished) = run(&["`", "`", "`", "\nSECRETO\n", "```\n", "Después. "]);
    for step in &pushed[..5] {
        assert_eq!(*step, Vec::<String>::new(), "{pushed:?}");
    }
    assert_eq!(pushed[5], owned(&["Después."]));
    assert_eq!(finished, Vec::<String>::new());
}

#[test]
fn test_tilde_fences_and_an_info_string_open_and_close_a_fence() {
    assert_eq!(all(&["Uno.\n~~~\nSECRETO\n~~~\nDos.\n"]), owned(&["Uno.", "Dos."]));
    assert_eq!(
        all(&["Uno.\n``` python\nSECRETO\n```\nDos.\n"]),
        owned(&["Uno.", "Dos."])
    );
    assert_eq!(
        all(&["Uno.\n   ```\nSECRETO\n   ```\nDos.\n"]),
        owned(&["Uno.", "Dos."]),
        "three spaces still open a fence"
    );
}

#[test]
fn test_four_spaces_before_a_fence_is_not_a_fence() {
    assert_eq!(all(&["    ```\nHola. Adiós.\n"]), owned(&["Hola.", "Adiós."]));
}

#[test]
fn test_a_fence_mark_inside_a_line_is_not_a_fence() {
    assert_eq!(
        all(&["Usa ``` para abrir. Y sigue.\nOtra.\n"]),
        owned(&["Usa para abrir.", "Y sigue.", "Otra."])
    );
    assert_eq!(
        all(&["Usa ~~~ aquí. Luego sigue.\nOtra.\n"]),
        owned(&["Usa aquí.", "Luego sigue.", "Otra."])
    );
}

#[test]
fn test_an_unclosed_fence_drops_its_content_and_the_next_reply_is_not_fenced() {
    let mut chunker = SentenceChunker::default();
    let mut said = chunker.push("Antes.\n```\nSECRETO sin cerrar\nmás SECRETO");
    said.extend(chunker.finish());
    assert_eq!(said, owned(&["Antes."]));
    assert_eq!(chunker.push("Nueva respuesta.\n"), owned(&["Nueva respuesta."]));
    assert_eq!(chunker.finish(), Vec::<String>::new());
}

#[test]
fn test_a_blank_partial_line_and_a_fence_prefix_are_held_until_the_newline() {
    let mut chunker = SentenceChunker::default();
    assert_eq!(chunker.push("  "), Vec::<String>::new());
    assert_eq!(chunker.push("``"), Vec::<String>::new());
    assert_eq!(chunker.push("`\n"), Vec::<String>::new(), "a fence opened");
    assert_eq!(chunker.push("SECRETO\n"), Vec::<String>::new());
    assert_eq!(chunker.push("```\n"), Vec::<String>::new(), "the fence closed");
    assert_eq!(chunker.push("Libre.\n"), owned(&["Libre."]));
}

#[test]
fn test_a_chunker_is_reusable_after_finish_and_finish_alone_says_nothing() {
    let mut chunker = SentenceChunker::default();
    assert_eq!(chunker.finish(), Vec::<String>::new());
    assert_eq!(chunker.push("Uno. Dos"), owned(&["Uno."]));
    assert_eq!(chunker.finish(), owned(&["Dos"]));
    assert_eq!(chunker.finish(), Vec::<String>::new());
    assert_eq!(chunker.push("Otra. "), owned(&["Otra."]));
    assert_eq!(chunker.push(""), Vec::<String>::new());
    assert_eq!(chunker.push("   "), Vec::<String>::new());
}

#[test]
fn test_a_sentence_over_the_queue_limit_is_split_between_words() {
    let sentence = format!("{}.", "palabra ".repeat(1_250).trim_end());
    assert!(
        sentence.chars().count() > 2 * TEXT && sentence.chars().count() < 3 * TEXT,
        "{}",
        sentence.chars().count()
    );
    let said = all(&[&sentence]);
    assert_eq!(
        said.len(),
        3,
        "{:?}",
        said.iter().map(|p| p.chars().count()).collect::<Vec<_>>()
    );
    assert!(said.iter().all(|piece| piece.chars().count() <= TEXT));
    assert_eq!(said.join(" "), sentence);
}

#[test]
fn test_a_word_longer_than_the_limit_is_truncated_to_it() {
    let said = all(&[&format!("{} fin.", "x".repeat(TEXT * 2))]);
    assert!(
        said.iter().all(|piece| piece.chars().count() <= TEXT),
        "{:?}",
        said.iter().map(|p| p.chars().count()).collect::<Vec<_>>()
    );
    assert_eq!(said[0], "x".repeat(TEXT));
    assert_eq!(said.last().map(String::as_str), Some("fin."));
}

#[test]
fn test_ten_thousand_characters_in_one_sentence_give_three_pieces() {
    let ten_k = format!("{}.", "ñandú ".repeat(1_667).trim_end());
    assert!(ten_k.chars().count() >= 10_000);
    let said = all(&[&ten_k]);
    assert_eq!(
        said.len(),
        3,
        "{:?}",
        said.iter().map(|p| p.chars().count()).collect::<Vec<_>>()
    );
}

#[test]
fn test_a_reply_arriving_a_character_at_a_time_gives_what_the_whole_gives() {
    let reply = "Hola. ¿Qué tal? Todo bien, gracias.\n- uno\n- dos\n```\nSECRETO\n```\nFin sin punto";
    let chars: Vec<String> = reply.chars().map(String::from).collect();
    let deltas: Vec<&str> = chars.iter().map(String::as_str).collect();
    assert_eq!(all(&deltas), all(&[reply]));
}
