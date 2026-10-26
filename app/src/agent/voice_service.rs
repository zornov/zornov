// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where spoken lines and described sounds come from.
//!
//! Narration is made by one of: the Mac's own voices, Fish Audio,
//! ElevenLabs or OpenAI; sounds described in words come from ElevenLabs.
//! The choice lives in `~/Movies/Studio/voice-services.json`, shared by the
//! window, the `zornov` command line and every agent, and every line goes
//! through [`speak`], so a choice made in Settings holds for all of them.
//!
//! Nothing falls back silently: when the chosen service fails, the line
//! fails and says why. The Mac voices can also be switched off outright
//! (Settings, or `STUDIO_NO_SYSTEM_VOICE=1` in the environment, which
//! wins over the file): then no agent can speak with them, even by asking
//! for one by name.

use super::keychain;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Who speaks the narration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// The voices installed on this Mac.
    #[default]
    Mac,
    Fish,
    ElevenLabs,
    OpenAi,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::Mac, Provider::Fish, Provider::ElevenLabs, Provider::OpenAi];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Mac => "Mac voices",
            Provider::Fish => "Fish Audio",
            Provider::ElevenLabs => "ElevenLabs",
            Provider::OpenAi => "OpenAI",
        }
    }

    /// The Keychain account of its API key.
    pub fn key_account(self) -> Option<&'static str> {
        match self {
            Provider::Mac => None,
            Provider::Fish => Some(keychain::FISH),
            Provider::ElevenLabs => Some(keychain::ELEVENLABS),
            Provider::OpenAi => Some(keychain::OPENAI),
        }
    }
}

/// The voice and sound choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub provider: Provider,
    /// Never speak with the Mac's own voices, whatever is asked.
    pub mac_voice_off: bool,
    /// Fish Audio voice model id (`reference_id`); empty: Fish's default.
    pub fish_voice: String,
    pub fish_model: String,
    pub elevenlabs_voice: String,
    pub elevenlabs_model: String,
    pub openai_voice: String,
    pub openai_model: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Mac,
            mac_voice_off: false,
            fish_voice: String::new(),
            fish_model: "s2.1-pro".into(),
            // "Rachel", one of ElevenLabs' standing voices.
            elevenlabs_voice: "21m00Tcm4TlvDq8ikWAM".into(),
            elevenlabs_model: "eleven_multilingual_v2".into(),
            openai_voice: "alloy".into(),
            openai_model: "gpt-4o-mini-tts".into(),
        }
    }
}

fn settings_path() -> PathBuf {
    std::env::var_os("STUDIO_VOICE_SETTINGS").map(PathBuf::from).unwrap_or_else(|| studio_paths::studio().join("voice-services.json"))
}

impl VoiceSettings {
    pub fn load() -> Self {
        std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = settings_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// Whether the Mac's voices may speak: off in Settings or by
    /// `STUDIO_NO_SYSTEM_VOICE` means never.
    pub fn mac_voice_allowed(&self) -> bool {
        !self.mac_voice_off && !forced_off()
    }
}

/// `STUDIO_NO_SYSTEM_VOICE` set to anything but empty, 0 or false.
pub fn forced_off() -> bool {
    std::env::var("STUDIO_NO_SYSTEM_VOICE").is_ok_and(|v| !matches!(v.trim().to_lowercase().as_str(), "" | "0" | "false" | "no"))
}

/// A spoken line: mono samples, and who spoke it.
pub struct Line {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub by: String,
}

const MAC_OFF: &str = "The Mac's built-in voices are switched off (Settings ▸ AI ▸ Voice & sound, or STUDIO_NO_SYSTEM_VOICE), so nothing was spoken. Use another voice service from the list, or tell the person it is not available";

/// Speaks `text` with the chosen service. `voice`: a voice of that
/// service (a Mac voice id, a Fish model id, an ElevenLabs voice id, an
/// OpenAI voice name); `words_per_minute`: only when asked for.
pub fn speak(text: &str, voice: Option<&str>, words_per_minute: Option<f32>) -> Result<Line, String> {
    speak_with(&VoiceSettings::load(), text, voice, words_per_minute)
}

pub fn speak_with(s: &VoiceSettings, text: &str, voice: Option<&str>, words_per_minute: Option<f32>) -> Result<Line, String> {
    // A Mac voice asked for by name never slips past the choice.
    let mac_id = voice.is_some_and(|v| v.starts_with("com.apple."));
    if mac_id && s.provider != Provider::Mac {
        // A Mac voice named while another service is chosen: that service
        // speaks, in its own voice.
        return speak_with(s, text, None, words_per_minute);
    }
    if s.provider == Provider::Mac {
        if !s.mac_voice_allowed() {
            return Err(MAC_OFF.into());
        }
        let sp = studio_hw::speech::speak(text, voice, Some(words_per_minute.unwrap_or(175.0)))?;
        return Ok(Line { samples: sp.samples, rate: sp.rate, by: voice.map(|v| v.rsplit('.').next().unwrap_or(v).to_owned()).unwrap_or_else(|| "Mac voice".into()) });
    }
    let key = s.provider.key_account().and_then(key_for).ok_or_else(|| format!("{} is not available. Nothing was spoken: use the Mac voices or another voice service", s.provider.name()))?;
    // Speed as a factor of a normal 175 words a minute.
    let speed = words_per_minute.map(|w| (w / 175.0) as f64);
    let pick = |own: Option<&str>, set: &str| own.filter(|v| !v.is_empty()).map(str::to_owned).unwrap_or_else(|| set.to_owned());
    match s.provider {
        Provider::Fish => {
            let voice = pick(voice, &s.fish_voice);
            let mut body = json!({"text": text, "format": "wav", "sample_rate": 44100, "latency": "normal"});
            if !voice.is_empty() {
                body["reference_id"] = json!(voice);
            }
            if let Some(sp) = speed {
                body["prosody"] = json!({"speed": sp.clamp(0.5, 2.0)});
            }
            let bytes = post(&format!("{}/v1/tts", base("https://api.fish.audio")), &[("model", s.fish_model.as_str())], &format!("Authorization: Bearer {key}"), &body)?;
            let (samples, rate) = from_wav(&bytes)?;
            Ok(Line { samples, rate, by: format!("Fish Audio{}", if voice.is_empty() { String::new() } else { format!(" · {voice}") }) })
        }
        Provider::ElevenLabs => {
            let voice = pick(voice, &s.elevenlabs_voice);
            if voice.is_empty() {
                return Err("ElevenLabs needs a voice id (Settings ▸ AI ▸ Voice & sound)".into());
            }
            let mut body = json!({"text": text, "model_id": s.elevenlabs_model});
            if let Some(sp) = speed {
                body["voice_settings"] = json!({"speed": sp.clamp(0.7, 1.2)});
            }
            let bytes = post(&format!("{}/v1/text-to-speech/{voice}?output_format=pcm_44100", base("https://api.elevenlabs.io")), &[], &format!("xi-api-key: {key}"), &body)?;
            Ok(Line { samples: from_pcm16(&bytes), rate: 44_100, by: format!("ElevenLabs · {voice}") })
        }
        Provider::OpenAi => {
            let voice = pick(voice, &s.openai_voice);
            let mut body = json!({"model": s.openai_model, "input": text, "voice": voice, "response_format": "pcm"});
            if let Some(sp) = speed {
                body["speed"] = json!(sp.clamp(0.25, 4.0));
            }
            let bytes = post(&format!("{}/v1/audio/speech", base("https://api.openai.com")), &[], &format!("Authorization: Bearer {key}"), &body)?;
            Ok(Line { samples: from_pcm16(&bytes), rate: 24_000, by: format!("OpenAI · {voice}") })
        }
        Provider::Mac => unreachable!(),
    }
}

/// Whether a hosted service is set up to make sounds from words.
pub fn sound_service_ready() -> bool {
    key_for(keychain::ELEVENLABS).is_some()
}

/// A sound described in words (`"glass shattering on concrete"`), made by
/// ElevenLabs: stereo-ready mono samples at 44.1 kHz.
pub fn described_sound(text: &str, seconds: Option<f64>) -> Result<Vec<f32>, String> {
    let key = key_for(keychain::ELEVENLABS).ok_or("Sounds described in words are not available. Use a made kind (riser, whoosh, boom, hit) or catalogue panel audio")?;
    let mut body = json!({"text": text});
    if let Some(d) = seconds {
        body["duration_seconds"] = json!(d.clamp(0.5, 22.0));
    }
    let bytes = post(&format!("{}/v1/sound-generation?output_format=pcm_44100", base("https://api.elevenlabs.io")), &[], &format!("xi-api-key: {key}"), &body)?;
    Ok(from_pcm16(&bytes))
}

/// The service address; tests point every service at a stand-in on this
/// machine with `STUDIO_VOICE_TEST_BASE` (only a local address counts, so
/// a key can never be sent elsewhere this way).
fn base(real: &str) -> String {
    match std::env::var("STUDIO_VOICE_TEST_BASE") {
        Ok(b) if b.starts_with("http://127.0.0.1:") => b.trim_end_matches('/').to_owned(),
        _ => real.to_owned(),
    }
}

/// The saved key; against a local stand-in, `STUDIO_VOICE_TEST_KEY`.
fn key_for(account: &str) -> Option<String> {
    if base("") != "" {
        if let Ok(k) = std::env::var("STUDIO_VOICE_TEST_KEY") {
            return Some(k);
        }
    }
    keychain::api_key(account)
}

/// POSTs JSON and returns the body. The secret header (`Name: value`)
/// stays in memory: no command line, no file.
fn post(url: &str, headers: &[(&str, &str)], secret_header: &str, body: &Value) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let (sk, sv) = secret_header.split_once(':').map(|(k, v)| (k.trim(), v.trim())).ok_or("a malformed key header")?;
    let mut all: Vec<(&str, &str)> = vec![("content-type", "application/json"), (sk, sv)];
    all.extend_from_slice(headers);
    let reply = studio_net::post(url, &all, &json, std::time::Duration::from_secs(180))?;
    let status = reply.status as u32;
    let bytes = reply.bytes(512 << 20).map_err(|e| format!("the reply broke off: {e}"))?;
    if !(200..300).contains(&status) {
        let msg = serde_json::from_slice::<Value>(&bytes).ok().map(|v| {
            v.get("message").or_else(|| v.get("detail").and_then(|d| d.get("message").or(Some(d)))).or_else(|| v.get("error").and_then(|e| e.get("message").or(Some(e)))).map(|m| m.as_str().map(str::to_owned).unwrap_or_else(|| m.to_string())).unwrap_or_else(|| v.to_string())
        });
        let what = match status {
            401 | 403 => "the key was refused",
            402 => "out of credit",
            404 => "that voice was not found",
            422 | 400 => "the request was refused",
            429 => "too many requests just now",
            _ => "the service failed",
        };
        return Err(format!("{what} ({status}{})", msg.map(|m| format!(": {}", m.chars().take(300).collect::<String>())).unwrap_or_default()));
    }
    if bytes.len() < 64 {
        return Err("the service sent back no sound".into());
    }
    Ok(bytes)
}

/// Where narration sits: broadcast speech loudness (about −16 LUFS, the
/// level podcasts and social video use), peaks kept under −1 dBFS.
pub const SPEECH_LUFS: f64 = -16.0;

/// Brings a spoken line to [`SPEECH_LUFS`] whatever service made it
/// (Fish Audio sends lines near −26, the Mac's voices near −20), so a
/// voice is never lost under the music because of where it came from.
/// Loudness is gated like BS.1770 (400 ms blocks, silence and quiet
/// breaths left out); loud syllables are softly limited.
pub fn level_speech(samples: &mut [f32], rate: u32) {
    let block = (rate as usize * 4 / 10).max(1);
    let hop = (rate as usize / 10).max(1);
    let mut energies = Vec::new();
    let mut i = 0;
    while i + block <= samples.len() {
        energies.push(samples[i..i + block].iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / block as f64);
        i += hop;
    }
    if energies.is_empty() {
        energies.push(samples.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / samples.len().max(1) as f64);
    }
    let loud: Vec<f64> = energies.iter().copied().filter(|e| *e > 1e-7).collect();
    if loud.is_empty() {
        return;
    }
    let mean = loud.iter().sum::<f64>() / loud.len() as f64;
    let kept: Vec<f64> = loud.iter().copied().filter(|e| *e > mean * 0.1).collect();
    let level = 10.0 * (kept.iter().sum::<f64>() / kept.len().max(1) as f64).log10() - 0.691;
    let gain = 10f64.powf(((SPEECH_LUFS - level).clamp(-20.0, 24.0)) / 20.0) as f32;
    // A soft knee from −4 dBFS, never past −1 dBFS.
    const KNEE: f32 = 0.63;
    const ROOM: f32 = 0.89 - KNEE;
    for x in samples.iter_mut() {
        let v = *x * gain;
        let a = v.abs();
        *x = if a <= KNEE { v } else { v.signum() * (KNEE + ROOM * ((a - KNEE) / ROOM).tanh()) };
    }
}

/// Mono samples from a WAV file's bytes.
fn from_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let w = studio_media::wav::parse(bytes).map_err(|e| format!("the sound that came back is not readable: {e}"))?;
    let ch = w.channels.max(1) as usize;
    Ok((w.samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect(), w.sample_rate))
}

/// Mono samples from raw 16-bit little-endian PCM.
fn from_pcm16(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_voices_can_be_switched_off_for_good() {
        let mut s = VoiceSettings { mac_voice_off: true, ..Default::default() };
        let e = speak_with(&s, "hello", None, None).err().unwrap();
        assert!(e.contains("switched off"), "{e}");
        // Asking for a Mac voice by name does not get past it.
        let e = speak_with(&s, "hello", Some("com.apple.voice.premium.en-US.Zoe"), None).err().unwrap();
        assert!(e.contains("switched off"), "{e}");
        // Another service chosen: a Mac voice named is not used, and with
        // no service the line fails rather than falling back.
        s.provider = Provider::Fish;
        if keychain::api_key(keychain::FISH).is_none() {
            let e = speak_with(&s, "hello", Some("com.apple.voice.premium.en-US.Zoe"), None).err().unwrap();
            assert!(e.contains("Fish Audio is not available"), "{e}");
        }
        assert!(!s.mac_voice_allowed());
        assert!(VoiceSettings::default().mac_voice_allowed() || forced_off());
    }

    #[test]
    fn settings_read_with_missing_fields() {
        let s: VoiceSettings = serde_json::from_str(r#"{"provider": "fish", "mac_voice_off": true}"#).unwrap();
        assert_eq!(s.provider, Provider::Fish);
        assert!(s.mac_voice_off);
        assert_eq!(s.fish_model, "s2.1-pro");
        let e: VoiceSettings = serde_json::from_str(r#"{"provider": "eleven_labs"}"#).unwrap();
        assert_eq!(e.provider, Provider::ElevenLabs);
    }

    #[test]
    fn quiet_and_loud_lines_come_out_at_speech_level() {
        let rate = 24_000;
        for amp in [0.02f32, 0.2, 0.9] {
            // Speech-like: bursts of a tone with pauses.
            let mut line: Vec<f32> = (0..rate * 3).map(|i| if (i / (rate / 4)) % 3 == 2 { 0.0 } else { amp * (i as f32 * 0.07).sin() * (1.0 + 0.5 * (i as f32 * 0.003).sin()) / 1.5 }).collect();
            level_speech(&mut line, rate as u32);
            let peak = line.iter().fold(0f32, |m, x| m.max(x.abs()));
            assert!(peak <= 0.891, "{amp}: peak {peak}");
            let voiced: Vec<f32> = line.iter().copied().filter(|x| x.abs() > 1e-4).collect();
            let rms_db = 10.0 * (voiced.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / voiced.len() as f64).log10();
            assert!((rms_db - SPEECH_LUFS).abs() < 2.5, "{amp}: {rms_db:.1} dB");
        }
    }

    #[test]
    fn pcm_and_wav_come_back_as_mono() {
        assert_eq!(from_pcm16(&[0, 0x40, 0, 0xC0]), vec![0.5, -0.5]);
        let w = studio_media::wav::Wav { sample_rate: 44100, channels: 2, samples: vec![0.5, 0.5, -0.25, -0.25] };
        let (m, r) = from_wav(&studio_media::wav::encode_pcm16(&w)).unwrap();
        assert_eq!(r, 44100);
        assert!((m[0] - 0.5).abs() < 1e-3 && (m[1] + 0.25).abs() < 1e-3);
    }
}
