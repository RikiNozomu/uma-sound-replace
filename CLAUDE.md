# uma-sound-replace

Hachimi Edge plugin (Rust cdylib, Windows x86-64, plugin API v3) for Umamusume (Global):
plays the user's own files (wav/mp3/flac/ogg) instead of the game's sounds (cues). Same
conventions as uma-lock-theater / uma-esport-spectator (`api.rs`, `settings.rs`, `sys.rs`).

## Layout
- `src/lib.rs`: `hachimi_init_v3`, settings + library scan at load, hook install at game ready
  (Hachimi on Windows inits plugins *after* firing game-initialized callbacks, so readiness is
  also checked at load), `reload()` (rescan + background decode), menu section.
- `src/cri.rs` (Windows): hooks on `cri_ware_unity.dll` native exports, found with
  GetProcAddress, hooked with Hachimi's interceptor (MinHook: one hook per function; Hachimi
  hooks only the managed `CriAtomExPlayer`/`CriAtomExAcb` wrappers, not these). Player -> cue
  (SetCueId/Name/Index; SetData/SetFile/SetWaveId/SetContentId clear it), ACB handle -> sheet name
  (LoadAcbFile path / LoadAcbData bytes, read lazily with `utf.rs`). At Start/Prepare: library
  lookup, then SetFormat(ADX=1) + SetNumChannels + SetSamplingRate + SetData on the same player,
  original Start. Our own calls go through the trampolines (`OURS` guards the trackers).
  `CriAtomExCueInfo` offsets (id@0, name@8, length@24, categories u16[16]@32) are verified once
  (`layout_ok`: name pointer == GetCueNameById(id)) before categories are copied
  (`criAtomExAcf_GetCategoryInfo` -> id@4 -> SetCategoryById), lengths patched in GetCueInfo*,
  and loops chosen (length < 0). Players whose categories the game sets itself are left alone.
  NOT verified in game yet (Nov 2026): exports, struct offsets, ADX via SetData.
- `src/library.rs` (pure, tested): `sounds\` scan (`<sheet starting snd_>\<cue>.<ext>` = that
  sheet only, any other place = any sheet; flac > wav > ogg > mp3), states Pending/Ready/Broken,
  background `prewarm`, `resolve` decodes on demand. ADX buffers are leaked (`&'static`): the
  game may still play them after a reload.
- `src/decode.rs` (pure, tested): Symphonia -> f32 -> mono/stereo, <= 48 kHz, i16.
- `src/adx.rs` (pure, tested): ADX type 3 encoder (v3 header, loop section, "(c)CRI", end
  frame 0x8001), reference decoder for tests.
- `src/utf.rs` (pure, tested): @UTF row-0 string column reader (ACB `Name`).
- `src/menu.rs`: Hachimi menu section (toggle, counts, broken files, Reload button via
  `api::extra()`, cue log with the last `RECENT_MAX` cues).
- `tools/list_cues.py`: every cue of the downloaded sound files to CSV (meta = SQLite3MC
  ChaCha20, key in the script; ACB @UTF CueNameTable/CueTable).

## Game data facts
- Voices: `sound/v/snd_voi_race_<chara><nn>.acb` cue `<sheet>_<cue_id:04>` (master
  `character_system_text`); commentary `sound/j/snd_voi_jky_{01,02,05}_*` cues
  `snd_voi_race_jky_<7 digits>`, commentator `snd_voi_cmt_{03,04}_*` cues `snd_voi_race_cmt_*`.
  The same cue name exists in several sheets (announcer sets).

## Rules
- Dependencies: symphonia only (pure Rust). Small OS calls are declared by hand in `src/sys.rs`.
- Host callbacks and hooks run plugin logic inside `catch_unwind` (release uses
  `panic = "unwind"`); a failure must fall back to the game's own sound.

## Commands
- `cargo test`
- Type-check the Windows build from Linux: `cargo check --release --target x86_64-pc-windows-msvc`
- `cargo build --release` (Windows, MSVC) -> `target/release/uma_sound_replace.dll`
