# Flow

Flow is a lightweight Windows dictation app built with Tauri 2, React, TypeScript,
Rust, and SQLite. Press Right Alt to record, press it again to finish, and Flow
transcribes, polishes, and pastes the result at the cursor in the application
that is active when recording ends.

## Requirements

- Windows 10 or 11 with WebView2
- Node.js 20 or newer
- Stable Rust with the MSVC toolchain
- A Groq API key for writing cleanup and optional Whisper transcription
- A Deepgram API key when using Nova-3 transcription

## Development

```powershell
npm install
npm run tauri dev
```

Groq and Deepgram API keys are entered in Flow's Settings and stored as generic
credentials in Windows Credential Manager. Settings also let you choose between
Deepgram Nova-3 and Groq Whisper Large V3 for speech-to-text; writing cleanup
continues to use Groq. Local history, dictionary entries, snippets, and preferences
are stored in Flow's application-data directory.

## Validation and release

```powershell
npm run typecheck
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

The release build produces an NSIS setup executable and an MSI package under
`src-tauri/target/release/bundle`.
