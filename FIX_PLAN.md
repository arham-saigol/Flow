# Branch verification and required fixes

Reviewed HEAD `7240fe2bb30e618c589f2f5c1bf5f6d428bec37c` against merge-base `9b89f94` at `origin/main`. The working tree was initially clean. The two branch commits are `750f766`, which adds PLAN.md, and `7240fe2`, which implements the changes in one commit. Scope is all 42 changed files, their callers, configuration, and the requirements in PLAN.md. Earlier merged fixes are baseline behavior, not additional branch commits.

The branch does not complete PLAN.md. Several added mechanisms are disconnected from their callers, and some regressions can lose recovery data or replace the clipboard without consent. This document is an implementation plan only. No application code was changed during review.

PLAN.md has R01 and R02 headings but refers repeatedly to R03. Interpret its R03 references as the verbatim prompt under R02. Do not invent another prompt or change the specified models.

## Evidence and limits

- Passed: frontend typecheck, 11 Vitest tests, production frontend build, 17 Rust tests with `--locked`, npm audit with zero vulnerabilities. `npm ls nanoid` resolves 3.3.18.
- Failed: rustfmt check and Clippy with `--all-targets -- -D warnings`. Clippy reports seven errors, listed in task 24.
- The Rust suite has 17 tests versus the baseline plan's 23. There are no backend workflow race, crash recovery, or mock HTTP integration suites. Passing tests do not establish the requested behavior.
- No live Groq requests, recording, clipboard delivery, installer execution, or installed-profile database manipulation was performed. No Flow process was running when checked.
- The referenced screenshot was not attached to the conversation available to the reviewer. A brief local-page DOM inspection occurred before the user prohibited browser/computer use. All such interaction stopped on that instruction. Native hang timing, rendered geometry, and taskbar behavior remain unverified on the affected instance.
- Locations below refer to this HEAD. Use the named functions as anchors after edits.

Additional verification used standalone command-line probes importing the unchanged Rust source modules, with synthetic databases/files under a unique temporary directory and HTTP servers bound only to 127.0.0.1. No Groq account or installed Flow profile was involved. These probes are evidence, not committed regression tests; turn their cases into the test suite during implementation.

| Probe | Observed result |
| --- | --- |
| Save two-word result, delete its history, invoke save_pending_to_history again | History reappeared; lifetime words increased from 2 to 4 |
| Update pending delivery to copied | History stayed not_attempted while pending became copied |
| Import valid synthetic PCM after making pending INSERT fail | Import returned Ok and deleted PCM despite the failed INSERT |
| Import PCM with a malformed terminal marker | Import returned Ok and created one pending row instead of quarantining |
| Correction source C#, .NET, don't, or foo-bar against identical input | None matched |
| Correction source four word against four, word | Incorrectly replaced the entire phrase and comma |
| One second of fixed-chunk resampling at 44.1/48/96 kHz, using the branch's delay trimming and no tail flush | Each produced 15,840 samples instead of 16,000 |
| Mock cleanup with missing finish_reason, tool calls, or two choices | All accepted |
| Mock cleanup with null content for filler input | Accepted as successful empty output |
| Mock cleanup with 32,001 bytes or embedded U+0001 | Both accepted |
| Mock Whisper text with no segment metadata | Accepted as nonsuspect; leading/trailing raw whitespace was removed |
| Open a synthetic version-0 database containing a 1 MiB payload, 258 original pages | Database::open took 13.613 seconds and migrated to version 2 |
| Query free space using the database filename versus its parent directory | Filename returned None; directory returned available bytes, confirming the current fail-open check |
| Exact prompt bytes compared with PLAN.md's fenced prompt | Identical, 3,967 bytes, LF and one trailing newline |
| ICO parsing/decoding through command-line image APIs | Six 32-bit frames at 16/24/32/48/64/256 pixels; decoded 32x32 frame has 976 opaque pixels |

Highest-priority implementation blockers are tasks 4-12: ownership, durable recovery, microphone teardown, gesture destination, and clipboard/delivery correctness. Tasks 1-2 address the reported startup/layout behavior. Task 3 remains a diagnosis gate rather than a claimed icon fix.

## Runtime problems

### 1. Startup stalls and UI-thread blocking

Evidence: `src-tauri/src/lib.rs`, setup around lines 437-480 and synchronous command handlers around lines 58-350; `database.rs::create_pre_upgrade_backup`, around lines 326-351; `platform.rs::copy_text`, `clipboard_owner`, and `paste_via_clipboard`.

New startup regression: setup runs database migration and recovery scanning synchronously before completing initialization. The backup copies only five SQLite pages per step and sleeps 250 ms between steps. The resolved rusqlite 0.32.1 implementation sleeps on each More/Busy/Locked result. A command-line call to the unchanged Database::open on a synthetic legacy database with a 1 MiB payload took 13.613 seconds. At a 4 KiB page size, a 10 MiB database requires roughly 128 seconds of intentional sleeps alone. This supplies a concrete path to first-upgrade "Not Responding" followed by eventual opening. It does not prove the user's particular hang had this cause.

Remaining runtime hazards: database/credential/device commands and recorder waits still execute synchronously. Copy can block the UI on CLIPBOARD_OPERATION while the paste worker owns that mutex and needs the main thread to render clipboard text or answer `window.hwnd()`. Recovery scanning adds synchronous file reads and FULL database commits at startup.

Exact fix:

1. Create an explicit initializing state. Show a lightweight loading window, complete expensive database/backup/spool work on a blocking worker, and publish ready only after initialization succeeds. Reject dictation and settings mutations while initializing.
2. Replace the five-page/250-ms backup loop with bounded batches on that worker, no unconditional sleep after successful progress, and a deadline for Busy/Locked retries. Keep capture disabled during backup. Report failures and keep the original DB.
3. Convert database, credential, enumeration, recorder, and clipboard IPC handlers to async wrappers. Run blocking implementations through `spawn_blocking` or their dedicated worker. Moving a function to `async` without moving its blocking body is insufficient.
4. Serialize clipboard operations on one background queue. Cache its owner HWND during main-thread setup and resolve it before locks. Keep subclass installation on the main thread. Never wait on a worker from the window procedure.
5. Test a synthetic multi-megabyte version-0 database, a locked database, and simultaneous Copy/paste/settings/refresh with injected blocking adapters. The main event loop must repaint throughout. Record startup stage durations to distinguish migration, spool import, native hooks, and WebView initialization.

### 2. Broken layout is introduced by undefined CSS classes

Evidence: `src/components/Dialog.tsx:29-58`, all new PrivacyNoticeModal and HistoryDetail markup, Dashboard and SettingsModal additions. `src/styles.css` is unchanged and contains handwritten classes. There is no Tailwind dependency, stylesheet import, or compiler configuration. Classes such as `fixed`, `inset-0`, `z-50`, `max-w-xl`, `flex`, `p-6`, and `overflow-y-auto` therefore do nothing.

The first-use dialog is a normal flex child inside `.app-shell`, rather than a fixed modal. It consumes layout space and competes with `.app-frame`. The root clips overflow. This is a direct source-level explanation for broken first-use layout. History rows also changed their markup while retaining CSS written for the old structure.

Exact fix:

1. Use the project's existing CSS approach. Replace undefined utility classes in the four affected components and Dashboard with named classes defined in styles.css. Include spacing, icon sizing, borders, colors, wrapping, and responsive sizing; do not repair only the outer backdrop.
2. Render shared dialogs into a dedicated portal sibling of the app root. Define a fixed inset backdrop, stacking order, viewport-constrained panel, scrollable body, and reachable footer. Use unique accessible title IDs.
3. Make the background inert, including toasts; restore its prior state on close. Handle a nested Privacy dialog in Settings with only the topmost dialog trapping focus. Restore focus to the actual connected enabled trigger.
4. Replace nested history-row buttons with a non-button container and separate details/Copy/Delete buttons. Store the clicked details trigger instead of assigning one ref to every row.
5. Preserve raw/final whitespace with `white-space: pre-wrap` and use `overflow-wrap:anywhere` for user strings. Add Settings tab arrow/Home/End navigation and roving tabindex. Move creation/edit dialogs onto the same implementation.
6. Acceptance requires first use and each dialog at 1080x720 and 780x600, long text, and 150/200% Windows scaling. Run visual/native acceptance only when permitted. DOM text tests alone do not test CSS geometry.

### 3. Missing taskbar icon needs a targeted diagnosis

There is no confirmed branch change causing this symptom. `tauri.conf.json` and icon files are unchanged. Only overlay sets `skipTaskbar: true`; main does not. The ICO exists. Resolved tauri-codegen 2.6.3 selects the configured ICO as the Windows default icon, tauri-build embeds it as resource 32512, and Tauri's window manager applies the default icon. Adding a duplicate `set_icon` call or changing overlay taskbar visibility is not an evidence-based fix.

Mechanical follow-up, without assuming a root cause:

1. Determine whether the missing object is the main taskbar button, its glyph, a stale pinned shortcut, or the system-tray icon. These are different paths.
2. Build the exact HEAD into a fresh artifact. The pre-existing debug Flow.exe inspected during review predates the implementation commit timestamp and was not launched. Do not assume it represents HEAD.
3. Inspect that artifact's RT_GROUP_ICON/RT_ICON resources, ICO frame dimensions/alpha, and installer shortcut target/AppUserModelID using command-line resource tools. Compare executable and shortcut identities.
4. In a future permitted native test, inspect main HWND owner, visibility, extended styles, and small/big icon handles. Ensure main retains ordinary taskbar eligibility and overlay remains excluded. Test normal, minimized, restored, second-instance, and installed launches.
5. If resource missing, fix the build resource input/linkage and rebuild installers. If runtime handles are absent despite valid resources, assign the validated default icon to main during setup and check errors. If shortcut identity is wrong, fix installer shortcut target/identity and test an upgrade. If only contrast is wrong, correct the offending icon frames. Do not delete global Windows icon caches as the default repair.
6. Keep this acceptance item open until a specific cause is demonstrated. Early tray absence during the slow setup in task 1 is plausible because create_tray runs afterward; it does not explain a missing main-window taskbar glyph by itself.

## Correctness and reliability fixes

### 4. Make session ownership authoritative

Evidence: `workflow.rs::cancel`, `report_error`, `process_captured_in_background`, `process_captured`, and `start`, around lines 123-319 and 575-693. Cancellation publishes Idle before the old operation finishes. Its eventual error calls unowned report_error, which clears a newer session. CapturedAudio contains neither session ID nor capture UUID; background completion reads the current coordinator's identity. Start publishes the native recording flag even when its conditional Starting transition no longer owns the session. Mutex-protected setup also performs credential, DB, filesystem, and native work inside the workflow lock.

Implement an owned session context carried through every recorder result and async continuation. It must include session ID, capture UUID, cancellation signal, stop reason, pending ID, and validated destination. Never substitute current state or a fresh UUID for a missing owner. Add one compare-and-transition helper and one owned finalizer. Reject stale results before persistence, state changes, events, or input. Command rejections must never finalize another operation. Keep cancellation occupied until work is quiescent, or invalidate it while guaranteeing all late work is rejected. Keep workflow locks confined to state transitions; use rollback/guard handling for every setup failure.

Fix `next_revision`: initial snapshot is revision 1 and the first fetch_add also returns 1. Every committed transition must have a strictly greater revision. Derive readiness from privacy, credentials, audio health, storage, and phase using cached validated readiness, not blocking reads inside snapshot locks. Refresh readiness when those inputs change. Hotkey/tray start and stop currently discard returned errors; report rejected operations visibly through the separate rejection path without clearing another session. Add barrier tests for cancel A/start B/late A error, stop-limit overlap, duplicate commands, failed start, and stale events.

### 5. Audio timeout must enter Faulted; microphone tests must actually stop

Evidence: `audio.rs:130-201`, recorder_worker around lines 252-319; `lib.rs:337-355`; `workflow.rs` start/stop error branches. Five-second receive timeouts return ordinary errors and reset Idle. Native work may later start a live stream. Cancel and StopMicTest acknowledge before dropping/joining their ActiveRecording. Faulted, MicrophoneTest, and ShuttingDown are declared but never committed.

Claim MicrophoneTest through the coordinator; retain its actual nonzero ID and pass it to Stop. `stop_mic_test(0)` must be removed. Confirm stream teardown before success. On any native deadline/disconnection, enter permanent Faulted for this process and reject new capture/test work, while leaving Copy/Discard usable. Reject late successful replies. Add a backend 30-second test deadline and stop on modal close/blur/hide. Emit the meter's session-tagged `audio-level` to main; current capture only emits `waveform` to overlay. Handle test stream failures against active_test. The test must neither spool nor upload audio. Test start/stop and a timed-out late reply without real hardware.

### 6. Preserve durable audio and implement the full terminal-marker protocol

Evidence: `audio.rs:655-690` ignores every spool write error; `RecoverySpool::write_pcm` ignores sync_data; normal finish never calls finalize_wav or sync_all; `workflow.rs` moves spool ownership into the worker and later cancellation takes an empty wf.spool. Successful completion leaves unmarked files behind. `scan_and_import_spools` ignores failed reads/inserts/marker writes and then deletes the only audio files.

Implement PLAN.md's ordered spool protocol as one serialized storage operation, shared by normal Stop, cancellation, import, completion, Discard, and expiry:

1. Keep a coordinator-owned immutable spool identity even while the worker owns its file handle. Create metadata with checked temporary-file/sync/write-through replacement.
2. Batch PCM writes, check writes and syncs, and stop on storage failure with a recoverable prefix. Flush and sync on Stop. Derive WAV/duration from complete samples. Do not maintain a second whole-recording in-memory WAV while spooling.
3. For cancellation, invalidate the session, durably mark cancelled before acknowledging, then stop the writer and remove its files. Marker failure must be reported. Worker teardown must precede cleanup.
4. Before import, validate filename UUID, metadata version/identity/rate/channels/bit depth, bounded file length, and every marker. Malformed markers quarantine the group; they must not fall through to import. Check both pending and history UUIDs.
5. Commit pending import before writing imported marker, then delete data only after marker durability. On any error preserve the last recoverable copy. Existing imported marker without a matching DB UUID is an inconsistency, not permission to erase data.
6. For completed/discarded/expired captures, make the appropriate marker durable before deleting the protecting pending row. Verify all PCM/metadata/UUID-scoped temporary files are gone before removing the marker. Do not swallow deletion errors or remove protection while files remain.
7. Add failure injection after every numbered storage operation in PLAN.md. Include failed INSERT, failed marker rename, failed unlink, cancelled capture restart, delivered capture restart, and deletion of its history before restart. Assert no loss, resurrection, duplicate UUID, or automatic processing.

### 7. Capture limit and partial capture must stop for review

Evidence: `audio.rs::spawn_capture_worker`, around lines 661-750, still exits only after an empty consumer pass while the producer remains live. CapturedAudio.partial is false on the five-minute cap. `process_captured` consequently chooses automatic delivery. Stream-failure partial audio immediately enters provider processing without confirmation.

Stop accepting samples and terminate the consumer deterministically at exactly 4,800,000 output samples. Send a typed limit stop reason with the original identity and let one worker finalize once. Limit, stream failure, overflow, and imported captures become copy_only pending review items. Do not call Groq, focus windows, or change clipboard on that path. Preserve the partial warning independently of transcription review status. Test continuous production at the cap and concurrent manual Stop; count one finalization and zero automatic delivery operations.

### 8. Capture the destination at the gesture, and keep the hook nonblocking

Evidence: `platform.rs::is_flow_window`, around lines 134-146, still falls through to Tauri HWND getters for every external HWND. `remember_target` takes a mutex. Hook callbacks call Tauri emit/spawn. `workflow.rs::stop_and_process` captures foreground only after asynchronous scheduling. Tray capture is not passed through to processing. PID is checked once but process creation identity and destruction tracking are absent.

Remove the Tauri fallback from hook-side identity checks. Cache handles during setup and treat missing cache as unavailable. Send immutable HWND/PID/cursor/session snapshots at stop key-up through a bounded 64-entry nonblocking queue, with coalescing and correct modifier release on overflow. Snapshot tray destination before menu focus changes. Validate process identity on a worker and again immediately before input. Flow/invalid/missing destinations require review and explicit Copy; never replace them with later foreground or LAST_TARGET. Add HWND destruction invalidation.

Handle GetMessageW positive/zero/minus-one separately, unhook both handles on every exit/install failure, and reset physical-key state on lock/unlock/resume. Test a stalled main thread and delayed command dispatch.

### 9. Integrate the clipboard helper safely and remove destructive fallback

Evidence: `clipboard_snapshot.rs::capture_clipboard_snapshot` has no production caller. `platform.rs::prepare_temporary_clipboard` still calls in-process capture_open_clipboard/GetClipboardData. New `paste_text`, around lines 469-500, converts every failure to copy_text, which empties the user's clipboard, including after unsupported formats or an uncertain paste.

Wire the helper into the actual snapshot path before replacing foreign reads. Return all-or-nothing supported HGLOBAL data; helper currently silently skips unsupported, oversized, unreadable, and failed-lock formats. Check enumeration errors. Bound encoded frames and decoded totals/counts; JSON arrays can be much larger than their raw byte count. Prefer the bounded binary frame protocol. Enforce the deadline through helper exit and pipe cleanup, not only receipt of a message. Spawn without a visible console.

Under an open clipboard, compare the helper's sequence number before replacement. Preserve later same-owner/user clipboard changes. Remove every automatic `copy_text` fallback. A preservation/target failure leaves ready recovery and offers explicit Copy. Retain one serial background queue from task 1. Test hung delayed rendering, helper timeout/crash/malformed frame, unsupported bitmap, large HTML/RTF, sequence replacement, and restoration failure.

### 10. Separate recovery processing, review, and delivery

Evidence: `workflow.rs::run_pending`, around lines 354-526, treats any existing raw_text as nonsuspect, ignores persisted review/no_content/mode, recomputes corrections, and calls copy_text after all copy_only retries. `accept_pending_transcript` clears the review flag before claiming a session and ignores a rejected retry. Forced retranscription is absent.

Implement the persisted/IPC action table in PLAN.md exactly:

- Retry processing claims an idle session and resumes only an ordinary permitted stage. It performs no clipboard or input operation; success leaves ready pending.
- Partial/storage-recovered audio requires warning confirmation before processing. Suspect raw text requires a full-text confirmation and session-scoped acceptance. Keep its warning and copy_only mode; do not clear the warning globally before claiming ownership.
- Ready final_text takes precedence over review; Copy final/Copy raw are explicit actions and do not remove pending. No-content exposes Copy raw/Discard only.
- Add retry_pending_transcription through Rust, registration, api.ts, types, and UI. Require WAV and history_saved=false. Preserve all old text until a valid replacement arrives; then replace raw and invalidate downstream fields atomically. Failure/cancellation leaves old text intact.
- On restart set every pending delivery_mode to copy_only. Persist safe stage/error/retry metadata on failure; current save_pending_error is never called.

Add tests that count zero clipboard writes on every retry/review/limit path and zero key reads for ready Copy. Suspect acceptance must not be bypassable with ordinary Retry.

### 11. Fix history idempotence and delivery metadata

Evidence: `database.rs::save_pending_to_history`, around lines 901-979, only short-circuits history_saved if history_id is Some. Migrated saved rows and explicitly deleted history have history_saved=true/history_id=NULL. Retrying recreates history and increments statistics. `save_pending_final` commits separately. update_pending_delivery updates only pending, leaving history with not_attempted. New paste errors are collapsed into copied and pending is deleted.

Treat history_saved as authoritative regardless of history_id; return an optional history reference without insertion. Commit validated final text, first history insertion, counters, history_saved and history_id together. Exclude no-content/unresolved review. Clear matching history_id on retention as well as explicit deletion, retaining history_saved.

Return typed DeliveryOutcome plus independent restoration warning from native delivery. Persist shortcut_sent immediately before input, then update pending and linked history metadata together. Mark post-input failures uncertain and retain ready pending with the check-destination warning. Delete pending only after durable history and spool completion protection. Never automatically resend input. Test legacy saved rows, delete-history-then-retry, restoration failure, partial input, and crash at the input boundary.

### 12. Serialize destructive actions with active ownership and retention

Evidence: discard_pending releases its state check before deleting; retry can claim the row in between. `delete_all_history(true)` bypasses coordinator checks. Hourly `run_maintenance(None)` can delete an active expired row. Expiry ignores returned UUIDs and does not protect spools.

Claim deletion/expiry through the same coordinator reservation mechanism as Retry. Reject active IDs through the full operation, not a check-then-unlock gap. Route bulk pending deletion through checked Discard for each eligible row. Feed active identity to maintenance with race-safe exclusion. Run startup/hourly/settings maintenance; serialize marker processing before expiry/import, preserve original creation time, and perform idle WAL checkpoint/truncate. Test expiry concurrent with retry, bulk deletion during processing, and failed marker persistence.

### 13. Make recovery quota and backup policy real

Evidence: start calls check_recovery_quota(0,0), so leftover spool/quarantine bytes never count. Import performs no quota or free-space check. GetDiskFreeSpaceExW receives the SQLite filename rather than the data directory and query failure is treated as permission to proceed. Backup failure is only printed; backup deletion swallows errors, and UI backup fields are never returned.

Implement a serialized ledger counting distinct UUIDs once and all physical WAV/spool/metadata/marker/quarantine bytes. Reserve one slot and 20,248,664 bytes before Start, reconcile measured usage, and release only after verified cleanup. Query the data directory/volume, surface query failures, and enforce PLAN.md's separate start/import/backup disk-space allowances. Keep original creation/expiry on import; do not reset old spool age to now. Paginate pending metadata 50 per page.

Block structural migration when the required backup fails. Make version-0 adoption transactional, validate existing columns, and propagate row decode errors instead of filter_map dropping them. Preserve future-version refusal. Implement checked seven-day backup deletion and expose actual backup path/expiry and separate allocated storage sizes. Test repeated migration/backup failure, full disk, stale spools, and 100-item boundary.

### 14. Validate actual Groq responses and bound requests

Evidence: `groq.rs::clean`, around lines 281-335, accepts missing finish_reason, ignores tool_calls and extra choices, treats null content as empty, omits final byte/control bounds. Both stages read the entire response before checking the 1 MiB limit. Transcription trims raw text and accepts missing segments/metadata as confident. Oversized transcript can bypass cleanup validation via a snippet.

Use a streaming bounded body reader for transcription, cleanup, and models listing. Accept exactly one choice, explicit stop, string content, and no refusal/tool calls. Validate 32,000 UTF-8 bytes and allowed controls before any snippet/clipboard path. Preserve original Whisper text separately; use trimmed views only for validation. Missing/nonfinite confidence metadata must enter suspect review. Empty transcription must persist no_speech recovery, not an endless retryable generic short-recording error. Distinguish TooShort/NoAudibleAudio/SuspectSpeech. Validate empty cleanup against both raw and corrected text using Unicode punctuation rules. Retain fixed model IDs, hidden/low reasoning, serialized two-field data input, and exact prompt. Restrict alternate-base-URL construction to tests; production construction must always use API_BASE.

Add local mock-server fixtures for every completion/status/metadata boundary listed in PLAN.md, including null/missing finish, tool calls, multi-choice, oversized streaming body, NUL/C0 controls, correction-induced substantive text, and absent segment fields.

### 15. Enforce cancellation and one stage deadline across HTTP retries

Evidence: groq.rs retries all timeout errors, grants 90 seconds per attempt, ignores HTTP-date Retry-After, and retries after a few seconds even when Retry-After exceeds 30. Cancellation is checked only outside the entire request/retry loop.

Pass session cancellation into provider calls and select it against send, response reads, and sleeps. Use one monotonic 90-second stage deadline across up to three allowed attempts and eight-second connect bounds. Retry only pre-transmission connect failures and the specified statuses; do not retry ambiguous read timeouts. Parse both Retry-After forms; a delay above 30 seconds returns persisted retry time. Use typed stage/status/code/request-ID errors and fixed local messages. Key testing stays within ten seconds and is labeled model-list authentication, not an end-to-end dictation test.

### 16. Correct literal matching and stage snapshots

Evidence: `text.rs::apply_corrections`, around lines 227-263, splits sources by whitespace but tokenizes input into word/nonword runs. A source `C#`, `.NET`, `don't`, or `foo-bar` cannot match its identical text. A source `four word` incorrectly matches `four, word` because arbitrary punctuation is skipped. Write uniqueness uses normalize_correction_source while matching/migration use normalize_key. Workflow rereads dictionary and snippets after awaits and ignores stored corrected_text on retry.

Implement the normalized-to-original byte-span matching described in F15. Collapse whitespace only; preserve literal punctuation. Use one normalization function for matching, writes, and conflict migration. Apply nonoverlapping longest matches once with original casing outside replacements. Take one dictionary/snippet snapshot at processing start and persist corrected/selected results before advancing. Reuse committed stages on retry. Test the four literal sources above, punctuation separation, decomposed accents, expanding lowercase, identifiers, overlapping phrases, and dictionary changes during provider work.

### 17. Finish the resampler and channel-selection implementation

Evidence: `audio.rs:559-744`. No tail flush occurs, so remaining input shorter than a chunk and filter-delayed output are lost. Resampler construction uses `.ok()` and processing errors are ignored, allowing wrong-rate fallback or discarded chunks. Channel selection uses the first 100 ms even if silent. Frame collection discards incomplete interleaved frames, and nonfinite samples are skipped rather than reported. No sane rate/channel bounds precede allocation. finish_recording uses wall-clock duration.

Prefer supported mono, otherwise preserve partial interleaved frames and buffer the first nontrivial 100 ms for deterministic RMS selection. Report nonfinite input as partial corruption without shifting channels. Validate rate/channel counts. Construct rubato with checked errors, use its requested input lengths and reusable buffers, flush partial input and filter tail on Stop, trim documented startup delay, and cap the expected output sample count. Compute duration from samples. Batch PCM writes rather than one filesystem write per sample. Add deterministic 16/44.1/48/96 kHz, antiphase, right-only-after-silence, tail-length, and 40 dB alias-rejection fixtures.

### 18. Resolve missing microphones without choosing a same-name replacement

Evidence: `audio.rs::select_device`, around lines 468-508, falls back to the first matching name after stable-ID lookup failure. Settings has no unavailable saved option or focus refresh.

Allow name migration only for a recognized legacy selection that uniquely matches one endpoint; persist the stable ID. A missing modern explicit ID must fail even if another same-name device exists. Add unavailable selection display, refresh on modal focus, and sanitized endpoint-read warnings. Test duplicate names and unplug/replug. Keep System default explicitly following the OS default.

### 19. Align IPC types, recovery controls, and frontend lifecycle

Evidence: src/types.ts invents uuid, delivery_mode, token counts, has_seen_privacy_notice, and AppConfig fields absent from Rust while omitting review/outcome/no_content/conflict fields. Dashboard around lines 136-266 offers Deliver only for ready rows but calls suspect-transcript acceptance, which rejects them; suspect rows only get ordinary Retry. It does not subscribe to active_pending_id. Error/no-content completion paths do not consistently refresh the dashboard.

Mirror Rust payloads exactly, including every workflow phase, capture_uuid, review/partial/no_content, delivery outcome/warning, history_saved, expiry, validation limits, and privacy version. Add serialization contract fixtures produced by Rust, not hand-invented frontend objects. Implement task 10's action table and full-text confirmation dialogs. Add visible main-window Stop/Cancel, faulted restart message, active row disabling, and mutation/focus refresh. Never claim delivered when a background command merely accepted work.

Give dashboard/settings/dictionary/snippet loads and key tests generation guards including catch/finally. Tie Connected to the exact trimmed key tested. Disable fields while loading/saving and guard duplicate submits immediately. Use explicit failed-load/retry states. History details must render actual delivery warnings and show Copy errors in the dialog. Confirm history deletion, bulk deletion, reset, and pending Discard before IPC.

### 20. Complete overlay subscription and shortcut capture

Evidence: useWorkflowState fetches before listener installation; Overlay mixes unordered legacy events with workflow events, never fetches a snapshot, does not clear isClosing on new retry, retains fabricated progress, and accepts late animation callbacks. Retry does not prepare/show the native overlay. Shortcut capture clears its flag on key-down, emits in the hook, accepts constituent chord keys, and has no idle ownership, blur/hide exit, or deadline. Settings listener promises can resolve after cleanup and leak; its cleanup effect also runs whenever either mode changes.

Subscribe and await installation before fetching a snapshot; keep only strictly newer revisions. Drive overlay appearance and phases from one session/revision protocol, tag waveform events, reset every display ref on new sessions, and cancel animation on idle/error/disposal. Replace elapsed-time percentages with indeterminate progress. Prepare/show the overlay when appropriate or provide a usable main-window indicator/Stop fallback.

Own shortcut capture with an idle-only token. Queue supported physical keys outside the hook, reject chords/AltGr, and finish only after key-up/all involved keys release. Cancel on Escape, blur, hide, close, or 15 seconds. Make async listener cleanup disposal-safe and keep microphone/shortcut lifecycle effects independent. Test subscription gaps, StrictMode, retry after dismiss, current-key capture, chords, and late listener installation.

### 21. Fix settings/credential transaction boundaries

Evidence: lib.rs::save_settings applies external changes before DB validation, reads old credentials with `.ok()`, lacks serialization, and ignores most rollback errors. Settings reads are separate locks. credentials.rs uses a 512 KiB bound and permits tab on read; has_api_key conflates every read failure with missing. Remove key is allowed during provider work.

Validate enums and microphone field bounds before side effects; read settings in one snapshot. Serialize mutation on a blocking worker. Distinguish credential NotFound from inaccessible/corrupt and stop replacement on the latter. Use CRED_MAX_CREDENTIAL_BLOB_SIZE, 2,560 bytes in the resolved windows 0.58.0 binding, and identical UTF-8/control/length validation for read/write/test. Preserve RAII/null checks. Block removal during provider use unless processing is cancelled. Apply staged external changes then commit DB, configure hotkey/tooltip afterward, report every rollback failure, and reload actual persisted/autostart state. Reject keybind changes during active capture or held selected key. Clear unsaved keys on close/save. Test all subsystem failures with fake credentials.

### 22. Complete dictionary/snippet conflict and limit UI

Evidence: Dictionary.tsx, Snippets.tsx, and EditableRow.tsx were not changed. Migrated conflicts are disabled in storage but appear ordinary in these pages. Dictionary maxLength remains 120 despite Rust 100/200 scalar limits. Update commands return unit and frontend retains unsanitized submitted values.

Read limits/enums from AppConfig, count with Array.from, mirror backend bounds, and preserve content whitespace. Display every disabled conflict group with reason and edit/delete resolution actions. Show the bounded Whisper vocabulary-subset disclosure. Return normalized saved rows on writes and replace UI state from them. Test astral Unicode boundaries, normalized conflicts including equal replacements, punctuation-only triggers, missing IDs, and re-enabling the remaining valid conflict member.

### 23. Enforce privacy, authorization, diagnostics, and shutdown

Privacy: App.tsx acknowledges only localStorage; Rust's privacy_notice_version setter is unused and Start never checks it. PrivacyNoticeModal names Qwen 2.5 32B and promises zero provider retention/strict privacy parameters without evidence. Add main-only versioned acknowledgement IPC persisted in settings, and gate every Start path on it. Display the actual configured models and Preview dependency. State that audio, raw/corrected text, and spelling hints go to Groq. Link current provider policy without guaranteeing retention; disclose local unencrypted data, seven-day pending storage, backup expiry, deletion limits, and Windows clipboard history/sync. Update README and persistent Settings disclosure.

Authorization: split capability files still grant broad core defaults; main retains frontend autostart permission/dependency. build.rs declares no app command manifest. get_dashboard/list_dictionary/list_snippets/get_settings/copy_text/start/stop/cancel/retry lack main-window guards. Add explicit permissions and guards for all sensitive IPC, allowing overlay only snapshot/readiness and necessary event listening. Remove unused autostart JS authority, keep Rust ownership, and regenerate schemas. Test every sensitive command from overlay.

Diagnostics: init runs inside setup after directory resolution; setup still ends in expect. Logging records arbitrary error strings with a substring blacklist and retains current plus three archives, exceeding the three-file policy. Replace with allowlisted structured version/stage/session/duration/code/request-ID fields, bounded records, and current plus two archives. Initialize before failing setup stages, provide native startup error reporting and explicit backup recovery mode, and make export deliberate and bounded. Do not log transcript/path-containing arbitrary messages. Test sensitive fixtures and repeated rotation.

Shutdown: tray Quit still calls app.exit immediately and no phase-specific shutdown handler exists. Add ShuttingDown admission control and a five-second cooperative finalization: preserve capture spool, cancel network/backoff, prevent delivery before commit, retain uncertain post-input status, and exit without cancelled markers. Handle Windows shutdown best-effort. Update tray label/enabled state from workflow. Test Quit in every phase and restart recovery.

### 24. Add the missing release gates and meaningful tests

Keep successful nanoid 3.3.18 remediation. Document the override and removal condition. Add package engines for Node 24, secret-file exclusions, and locked Cargo CI. Add rustfmt, Clippy, pinned cargo-audit, secret scan, forbidden-provider scan, license inventory, and actual Windows packaging jobs. Existing CI does not run these required checks.

Fix Clippy failures mechanically after functional edits: reduce RecorderCommand's large variant with boxed owned payload, use `.first()`, simplify the helper's nested bounds check, replace the history tuple with a named row type, derive enum Defaults, and implement coordinator Default. Run rustfmt rather than suppressing its check.

Add fake audio, provider, credentials, storage failures, clock, clipboard/input, and event sink adapters. Coordinator tests must run without AppHandle/hooks. Add crash tests and mock HTTP contracts from tasks 4-23. Current cleanup corpus tests only count/category-check fixture strings; they never test a model response. Extend each fixture with mappings, intended meaning, protected facts, permitted edits, and reference output, including injection/mixed-language/negation cases from PLAN.md. Compare the entire prompt bytes with an independent snapshot and the plan's prompt, including LF/trailing newline.

Create opt-in synthetic WAV/model smoke tests for exact models and reasoning fields and the three-run semantic evaluation. Do not use installed credentials or waive failures. Add a test-only isolated data root. Implement release signing/timestamp/signature verification for EXE/NSIS/MSI, checksums, release notes, and manual updates; block publication when signing or live model gates are unavailable. Complete the specified Windows install/upgrade/uninstall/second-instance matrix, 100-dictation soak, and 24-hour idle/retention/handle test before calling the release complete. These are unperformed release gates, not claimed test failures.

## PLAN.md item-by-item disposition

Partial means some code exists but the full requirement is not satisfied. No item should be checked complete solely because its filename or type exists.

| Item | Verified disposition | Fix tasks |
| --- | --- | --- |
| R01 | Request model IDs/fields and two-field JSON implemented; validation, live smoke and test-only alternate URL restriction incomplete | 14, 15, 24 |
| R02 / references to R03 | Exact prompt bytes verified; committed substring test is not a full snapshot; live semantic evaluation absent | 24 |
| F01 | Coordinator exists; owner/cancellation/revision invariants broken | 4, 5 |
| F02 | Recorder commands tagged; CapturedAudio/completion ownership missing | 4, 7 |
| F03 | Partial-prefix rescue attempted; warning/confirmation and persistence incomplete | 6, 7, 14 |
| F04 | Spool and FULL SQLite added; durability/import/cleanup unsafe | 6 |
| F05 | Receive deadlines added; Faulted/teardown/deterministic cap absent | 5, 7 |
| F06 | PID field added; gesture snapshot and identity validation incomplete | 8 |
| F07 | Single Discard check added; races/bulk deletion/expiry bypass it | 6, 12 |
| F08 | Cancellation token added; durable cancellation and safe Quit absent | 4, 6, 15, 23 |
| F09 | HWND cache added; blocking fallback/hook operations remain | 8 |
| F10 | Blocking IPC paths remain | 1 |
| F11 | Helper added but unused and unsafe to integrate unchanged | 9 |
| F12 | Some completion checks and Unicode removal implemented; validation/delivery semantics fail | 9-11, 14 |
| F13 | 200-byte newest-first whole-entry selection implemented; UI note and full fixtures absent | 22, 24 |
| F14 | Language omission implemented; missing metadata/review/no-speech handling fail | 10, 14 |
| F15 | Local corrections/conflict migration exist; literal matching and persisted snapshots fail | 16, 22 |
| F16 | Core C#/.NET/C++ and punctuation-only tests pass; conflict resolution UI incomplete | 22, 24 |
| F17 | Retry loop/status messages exist; budget/cancellation/classification fail | 15 |
| F18 | Rubato/channel choice added; frame/tail/error handling and audio fixtures missing | 17 |
| F19 | Default fallback removed; same-name fallback and test lifecycle still fail | 5, 18 |
| F20 | Hourly pruning/constants added; actual quota/expiry/active exclusions incomplete | 12, 13 |
| F21 | Version-2 transaction/conflict columns implemented; adoption/backup safety incomplete | 13 |
| F22 | Keyset paging/details/delete APIs added; semantics/layout/confirmation incomplete | 2, 11, 19 |
| F23 | Validation enums added but late; serialized save/rollback/reconciliation absent | 21 |
| F24 | RAII/null/UTF-8/remove added; status/limit/removal policy incomplete | 21 |
| F25 | Backend scalar/count/control bounds added; frontend and normalized returns incomplete | 22 |
| F26 | Workflow listener added; retry/ordering/reset/lifecycle still broken | 20 |
| F27 | Generation/order/input guards not implemented | 19 |
| F28 | Boolean capture mode added; owner/key-up/chord/deadline policy missing | 20 |
| F29 | Shared Dialog exists; no working utility CSS/portal/inert/roving tabs | 2 |
| F30 | Capabilities split and some guards added; command authorization incomplete | 23 |
| F31 | Logger/export added; structured privacy/startup recovery/tray behavior incomplete | 1, 23 |
| F32 | Dependency fix verified; maintenance documentation missing | 24 |
| F33 | Toolchain/test/CI skeleton added; required failure/release gates absent | 24 |
| F34 | Notice and some deletion controls added; consent/policy/storage disclosure incorrect | 23 |

Fixed product decisions 1-4 are represented by the Windows project, fixed Groq models/options, and prompt. Decisions 5-9 fail at limit/recovery/delivery boundaries described above. Decision 10's language omission is implemented. Decision 11's retention enforcement is incomplete. No background telemetry, cloud sync, provider fallback, or automatic model-switching path was found for decision 12. Authenticated provider availability remains unverified.

## Implementation sequence and completion rule

1. Fix startup/layout and add isolated adapters/regression fixtures. Keep taskbar diagnosis separate until evidence identifies its cause.
2. Implement storage protocol, quota/migration safety, and history idempotence before changing coordinator completion paths.
3. Repair session ownership, audio/test/limit behavior, cancellation/shutdown, and native delivery together.
4. Finish provider validation/retries, correction matching, and audio DSP with deterministic fixtures.
5. Align IPC and frontend actions, privacy/permissions/settings, and diagnostics.
6. Run all PLAN.md CI commands with locked dependencies, then its live/native/release acceptance gates when their prerequisites and permission are available. Mark every table item complete only with its regression evidence. Keep unrun gates and unresolved taskbar diagnosis explicitly open.
