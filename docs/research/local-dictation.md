# Local dictation

Implementation for [issue #12](https://github.com/dergigi/goop/issues/12).

## Composer

The microphone starts dictation. First use offers a 487 MB model download, with progress, cancellation, and retry. It does not request microphone permission until recording starts. There are no downloads or model initialization during normal startup.

The recording footer follows [Paseo's controls](https://github.com/getpaseo/paseo/blob/d0bc188a5b1be72f170f7db850716b7006f12f7e/packages/app/src/components/dictation-controls.tsx): discard, level meter, elapsed time, insert into draft, and transcribe-and-send. Processing shows a spinner. The text box remains editable. Cmd+D (Ctrl+D on Windows/Linux) starts recording; pressing it again or pressing Enter transcribes and sends. Escape in the composer stops an active recording and transcribes into the draft without sending. The × button discards the recording. Shift+Enter still inserts a newline.

Transcripts append to the current draft and use the existing local persistence and NIP-37 sync. Sending requires the explicit send control, the same draft/replies/attachments, and focus still in that chat. Otherwise the result stays in the draft. Account changes discard stale results. Cancelling or closing the chat stops capture and ignores any late result.

Recordings last up to five minutes. At the limit, captured speech is transcribed into a draft. A disconnected or stalled microphone also preserves and transcribes any audio already captured. Recognition failures retain audio in memory for retry until discarded or the chat closes. Audio is never uploaded, saved to disk, or included in logs. Text follows the user's normal draft-sync settings.

## Runtime and storage

- Native microphone capture: CPAL 0.16, Core Audio / WASAPI / ALSA.
- Recognition: official sherpa-onnx Rust API 1.13.8, static native libraries, CPU execution provider, up to four threads.
- Model: NVIDIA Parakeet TDT 0.6B v3, INT8 ONNX conversion distributed by sherpa-onnx. Automatic language detection for 25 European languages.
- Workers handle permissions, capture, downloads, extraction, model loading, and recognition. The UI receives progress and results.
- One dictation worker at a time. Audio buffering is bounded. Inference uses segments up to 30 seconds, preferring a quiet boundary after 25 seconds. Cancellation is checked between segments; an active native decode must finish before its resources are released.
- The model unloads after completion or cancellation. It remains loaded while retryable audio is retained.
- Model files live under `common::support_dir()/speech/parakeet-v3-int8`. Settings → Local dictation → Remove model removes them. Removal runs in the background and is blocked while the model is in use.
- A filesystem lock also protects against concurrent Goop processes. Downloads use a staging directory, verify the complete archive's pinned size and SHA-256, extract only the four expected regular files, and rename the completed directory into place. Interrupted downloads restart cleanly.

### Model attribution

[NVIDIA Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) is licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). The INT8 ONNX conversion is distributed by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). Goop includes this attribution with the installed model.

[Download archive](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2): 487,170,055 bytes; SHA-256 `5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf`.

Model files occupy about 642 MiB. Installation temporarily needs space for both the archive and extracted model, about 1.1 GiB.

## Validation

Development Mac: Apple M4 Pro, 2026-10-08. Measurements use the actual INT8 model and bundled native runtime. No microphone was opened by automated tests.

- English model fixture: model load 0.81 s; transcription 0.27 s.
- German synthetic speech: load 0.82 s; transcription 0.40 s; expected sentence recognized.
- Portuguese synthetic speech: load 0.76 s; transcription 0.45 s; expected sentence recognized.
- 38.45 s repeated English fixture: load 0.82 s; transcription 5.42 s, exercising segmentation. Peak resident set about 1.48 GB; macOS reported peak memory footprint about 1.06 GB. These are test-process measurements, not total Goop memory usage.
- Silent audio produced an empty transcript, exercising the retry/discard path.
- The complete prefetched archive passed the production installation path: digest, extraction, attribution, atomic publication, cleanup, and reuse without another download.
- 81 deterministic speech, chat UI, and workspace tests passed.
- Unit coverage includes downmixing and bounded audio, segment boundaries, malformed/incomplete archives, cancellation, filesystem locking, preservation of edited drafts, and send/account guards.

Run deterministic tests with `cargo test -p speech -p chat_ui -p workspace --lib`. Hardware/model tests are explicitly ignored by default:

```sh
GOOP_SPEECH_MODEL_ROOT=/path/to/speech \
GOOP_SPEECH_WAV=/path/to/fixture.wav \
GOOP_SPEECH_EXPECT='expected words' \
cargo test -p speech real_model_transcribes_fixture -- --ignored --nocapture

GOOP_SPEECH_ARCHIVE=/path/to/model.tar.bz2 \
cargo test -p speech installs_verified_archive_fixture -- --ignored --nocapture

GOOP_DOWNLOAD_ROOT=/path/to/test-models \
cargo test -p speech downloads_verified_model -- --ignored --nocapture
```

User validation: the optimized macOS build was installed, the user tested dictation and the final keyboard behavior, and confirmed it was working before requesting release on 2026-10-09. Source CI passed on Linux, macOS, and Windows. Remaining manual checks: microphone denial and capture on other platforms, Flatpak capture, slower hardware, and varied accents. The development Mac's Rust network requests timed out while curl succeeded, with Little Snitch active, so that earlier automated network test did not pass. The user subsequently tested the feature in the installed app. The same archive was downloaded with curl and its pinned digest verified for recognition tests.
