# Dictation Reliability Fixes

## Goal

Make dictation cleanup faithful to the speaker, make automatic pasting dependable, and ensure every hotkey press receives visible, deterministic handling. Dictation text may appear in Windows clipboard history; preserving and restoring the user's current clipboard value is still required.

## Implementation

1. **Preserve dictation meaning** (`src-tauri/src/groq.rs`)
   - Rewrite the cleanup prompt to allow punctuation, formatting, and structural cleanup while explicitly forbidding omitted details, changed meaning, unsupported substitutions, or subjective removal of “fluff.”
   - Add a conservative fidelity safeguard so a suspiciously destructive cleanup is rejected or falls back to the raw transcript rather than being accepted merely because it is non-empty.
   - Stop unconditionally forcing transcription language to English. Use automatic detection or a configurable language while retaining dictionary spelling guidance.
   - Add tests for detail preservation rules, destructive-output fallback, and language request behavior.

2. **Simplify and harden paste delivery** (`src-tauri/src/platform.rs`, `src-tauri/src/workflow.rs`)
   - Replace the delayed-render/process-family authorization flow with a normal clipboard paste: preserve the existing clipboard, write the dictation eagerly as Unicode text, send `Ctrl+V`, then restore the previous clipboard after the destination has consumed it.
   - Remove clipboard-history/cloud exclusion markers; it is acceptable for dictations to appear in Windows clipboard history.
   - Do not make multiline paste depend on losslessly preserving every clipboard format or fall back to refusing the paste. Handle unsupported/large existing clipboard data with a reliable, documented fallback while avoiding destruction of newer clipboard content written concurrently by the user or another app.
   - Retain focus, modifier-release, input-injection, ownership/race, and clipboard-restoration safety checks where they do not prevent valid target applications from pasting.
   - Add unit tests for clipboard text encoding and restoration decisions, plus focused tests for unsupported clipboard formats and multiline delivery paths.

3. **Make hotkey handling deterministic** (`src-tauri/src/workflow.rs`, and UI event handling where needed)
   - Replace the silent `busy` no-op with an explicit state-machine outcome. A press during startup or processing must either be safely queued or produce immediate visible feedback explaining that Flow is busy.
   - Prevent rapid presses from being lost in the interval before the recorder reports `is_recording()`.
   - Add state-transition tests covering idle, starting, recording, processing, rapid repeated presses, and failures.

## Verification

- Run `npm run typecheck`, `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml`, and `cargo test --manifest-path src-tauri/Cargo.toml`.