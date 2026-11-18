use std::time::Instant;

use reqwest::multipart::{Form, Part};
use serde_json::Value;

use crate::app::{self, wav};
use crate::fake::{batch, nodes, ser};

const LOUD: f32 = 0.9;
const SEGMENTS: usize = 500;
/// FNV-1a 64 of each body as main f9ab90f rendered it, node off.
const EXPECTED: &[(&str, u64)] = &[
    ("openai json tags=false", 0x23a2_00c3_f046_9605),
    ("openai verbose_json tags=false", 0xd2c9_4b70_d0d9_24f2),
    ("openai text tags=false", 0xc790_200e_bdcc_e177),
    ("openai srt tags=false", 0x31ac_9e2d_746c_9e05),
    ("openai vtt tags=false", 0xf37a_5d62_9324_8e42),
    ("deepgram tags=false", 0x9608_3c68_c652_9234),
    ("scribe tags=false", 0x6271_6a63_b20e_382f),
    ("openai json tags=true", 0x1b06_b6b5_9ed9_3b25),
    ("openai verbose_json tags=true", 0x4698_dccf_865a_c806),
    ("openai text tags=true", 0x04bc_f216_c06b_0097),
    ("openai srt tags=true", 0x788c_3221_048e_6ab9),
    ("openai vtt tags=true", 0xe91d_634a_4a5f_6aea),
    ("deepgram tags=true", 0xea37_7fa0_c3dc_7ad8),
    ("scribe tags=true", 0x3ab1_78eb_7caa_3d93),
];

/// 500 utterances of varied length, each after a gap, from a fixed generator.
fn meeting() -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as u32
    };
    let parts: Vec<(f32, u32)> = (0..SEGMENTS)
        .flat_map(|_| [(0.0, 8_000 + next() % 16_000), (LOUD, 3_000 + next() % 30_000)])
        .chain([(0.0, 8_000)])
        .collect();
    wav(16_000, &parts)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn masked(mut body: Value) -> Vec<u8> {
    body["metadata"]["request_id"] = Value::Null;
    body["results"]["utterances"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .for_each(|utterance| utterance["id"] = Value::Null);
    serde_json::to_vec(&body).unwrap()
}

async fn render(tags: bool, timings: &mut Vec<(String, f64)>) -> Vec<(String, Vec<u8>)> {
    let (address, _) = app::serve(nodes(batch(0), Some(ser(0)), None), 1 << 28, tags).await;
    let client = reqwest::Client::new();
    let audio = meeting();
    let mut bodies = Vec::new();
    for format in ["json", "verbose_json", "text", "srt", "vtt"] {
        let form = Form::new()
            .text("model", "whisper-1")
            .text("response_format", format)
            .part("file", Part::bytes(audio.clone()).file_name("meeting.wav"));
        let started = Instant::now();
        let body = client
            .post(format!("http://{address}/v1/audio/transcriptions"))
            .multipart(form)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        timings.push((format!("openai {format} tags={tags}"), started.elapsed().as_secs_f64()));
        bodies.push((format!("openai {format} tags={tags}"), body.to_vec()));
    }
    let started = Instant::now();
    let body: Value = client
        .post(format!("http://{address}/v1/listen?language=es"))
        .header("content-type", "audio/wav")
        .body(audio.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    timings.push((format!("deepgram tags={tags}"), started.elapsed().as_secs_f64()));
    bodies.push((format!("deepgram tags={tags}"), masked(body)));
    let form = Form::new()
        .text("model_id", "scribe_v1")
        .part("file", Part::bytes(audio).file_name("meeting.wav"));
    let started = Instant::now();
    let body = client
        .post(format!("http://{address}/v1/speech-to-text"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    timings.push((format!("scribe tags={tags}"), started.elapsed().as_secs_f64()));
    bodies.push((format!("scribe tags={tags}"), body.to_vec()));
    bodies
}

#[tokio::test(flavor = "multi_thread")]
async fn test_five_hundred_segments_render_the_same_bytes_on_every_surface() {
    let mut timings = Vec::new();
    let mut bodies = render(false, &mut timings).await;
    bodies.extend(render(true, &mut timings).await);
    let sample: String = bodies
        .iter()
        .map(|(_, body)| String::from_utf8_lossy(body).into_owned())
        .collect();
    assert!(sample.matches("00:").count() > SEGMENTS);
    assert!(sample.contains("[happy] batch:"), "tags=true must tag the text");
    for (name, body) in &bodies {
        println!("RENDER_HASH {name} {:016x} {}", fnv(body), body.len());
    }
    for (name, seconds) in &timings {
        println!("RENDER_TIME {name} {seconds:.4}");
    }
    let moved: Vec<&str> = EXPECTED
        .iter()
        .filter(|(name, hash)| {
            bodies
                .iter()
                .find(|(seen, _)| seen == name)
                .is_none_or(|(_, body)| fnv(body) != *hash)
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(moved.is_empty(), "bodies that moved: {moved:?}");
    assert!(
        EXPECTED.is_empty() || EXPECTED.len() == bodies.len(),
        "every body needs its golden"
    );
}
