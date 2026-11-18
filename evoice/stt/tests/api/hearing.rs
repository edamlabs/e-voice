use std::num::NonZeroU32;
use std::time::Duration;

use e_voice_core::schema::lang::Lang;
use e_voice_daemon::hearing::base::{Hearing, Recognized, Transcription};
use e_voice_daemon::hearing::stt::{SttEvent, SttHearing, SttVoice};
use tokio_tungstenite::tungstenite::Message;

use crate::app::{self, exchange};
use crate::fake::{batch, nodes, ser};

async fn base() -> String {
    let (address, _) = app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 20, false).await;
    format!("ws://{address}")
}

fn rate() -> NonZeroU32 {
    NonZeroU32::new(16_000).unwrap()
}

fn blocks(value: f32, samples: usize) -> Vec<f32> {
    vec![value; samples]
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_daemons_url_is_accepted_and_every_event_it_gets_parses_into_the_mirror() {
    let base = base().await;
    let url = SttHearing::new(base).url(rate()).unwrap();
    let f32le =
        |samples: &[f32]| Message::Binary(samples.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>().into());
    let frames = vec![
        f32le(&blocks(0.0, 1_600)),
        f32le(&blocks(0.9, 8_000)),
        f32le(&blocks(0.0, 1_600)),
        Message::Text(r#"{"type":"end"}"#.into()),
    ];
    let (texts, code) = exchange(url.as_str(), frames).await;
    assert_eq!(code, Some(1000), "{texts:?}");
    let parsed: Vec<SttEvent> = texts.iter().map(|text| serde_json::from_str(text).unwrap()).collect();
    assert_eq!(
        parsed.first(),
        Some(&SttEvent::Speech {
            state: SttVoice::Started
        }),
        "{texts:?}"
    );
    assert!(
        parsed.contains(&SttEvent::Speech {
            state: SttVoice::Stopped
        }),
        "{texts:?}"
    );
    assert!(parsed.contains(&SttEvent::Closed {}), "{texts:?}");
    let finals: Vec<_> = parsed
        .iter()
        .filter_map(|event| match event {
            SttEvent::Final {
                text,
                lang,
                emotion,
                error,
            } => Some((text.as_str(), *lang, emotion.label, error.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(finals.len(), 1, "{texts:?}");
    assert_eq!(finals[0].0, "batch:8000");
    assert_eq!(finals[0].3, None);
    assert!(matches!(finals[0].1, Lang::Es | Lang::En));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_the_driver_against_the_real_service_hears_started_then_the_final_of_what_it_sent() {
    let hearing = SttHearing::new(base().await);
    let mut session = hearing.open(rate()).await.unwrap();
    session.send(&blocks(0.0, 1_600)).await.unwrap();
    session.send(&blocks(0.9, 8_000)).await.unwrap();
    session.send(&blocks(0.0, 16_000)).await.unwrap();
    let mut seen = Vec::new();
    while let Ok(Some(next)) = tokio::time::timeout(Duration::from_secs(5), session.next()).await {
        let recognized = next.unwrap();
        let last = matches!(recognized, Recognized::Final(..));
        seen.push(recognized);
        if last {
            break;
        }
    }
    assert!(matches!(seen.first(), Some(Recognized::Started)), "{seen:?}");
    match seen.last() {
        Some(Recognized::Final(heard, None)) => assert_eq!(heard.text, "batch:8000"),
        other => panic!("{other:?} in {seen:?}"),
    }
}
