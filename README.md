# uma-sound-replace

Hachimi Edge plugin for Umamusume: Pretty Derby (Steam, Windows, Global client): **play your
own sound files instead of the game's**: music, character voices, race commentary, effects.

- Put a `wav`, `mp3`, `flac` or `ogg` file in the plugin's `sounds` folder, named after the
  game sound (cue) it replaces. The game plays it instead.
- No file, or a file that cannot be read: the game's own sound plays, as usual.
- The game's own player plays the file, so the game's volume settings, pause and stop still
  apply. Music that loops in the game loops.

## Requirements

- The game on Steam with [Hachimi Edge](https://hachimi.noccu.art) installed and working
  (its `hachimi` folder exists in the game folder).
- To build: [Rust](https://rustup.rs) with the MSVC toolchain
  (`rustup default stable-x86_64-pc-windows-msvc`) and the
  [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  ("Desktop development with C++").

Paths below assume the default Steam location; the game folder is
`C:\Program Files (x86)\Steam\steamapps\common\UmamusumePrettyDerby` (called `<game>`).

## Install

**1. Build the plugin.**

```powershell
git clone https://github.com/RikiNozomu/uma-sound-replace
cd uma-sound-replace
cargo build --release
```

This produces `target\release\uma_sound_replace.dll`.

**2. Copy the DLL into Hachimi's folder** (close the game first: the DLL is locked while it runs).

```powershell
copy target\release\uma_sound_replace.dll "<game>\hachimi\"
```

**3. Tell Hachimi to load it.** In `<game>\hachimi\config.json`, add the DLL to
`load_libraries` (keep any plugins already listed there):

```json
"load_libraries": [
    "hachimi\\uma_sound_replace.dll"
]
```

**4. Start the game and check.** `<game>\hachimi.log` should contain
`[uma-sound-replace] plugin loaded` and `audio hooks installed`. The Hachimi menu has a
**Sound replace** section, and the folder `<game>\hachimi\uma-sound-replace\sounds\` exists.

## Use

**Name a file after the cue it replaces**, e.g. Special Week's first race voice line is cue
`snd_voi_race_100100_0000` in cue sheet `snd_voi_race_100100`:

```
<game>\hachimi\uma-sound-replace\sounds\
    snd_voi_race_100100_0000.wav                      any cue sheet with this cue name
    snd_bgm_AM003.mp3
    my commentary\snd_voi_race_jky_1000101.flac       any folder name works for this
    snd_voi_jky_02_1000000\snd_voi_race_jky_1000101.wav   only in this cue sheet
```

- A file anywhere in `sounds` (any sub-folders) replaces that cue name in every cue sheet.
- A file in a folder named like a cue sheet (starting with `snd_`) only replaces it in that
  sheet, and wins over the first kind. Some cue names exist in several sheets: the race
  commentary has three announcer sets (`snd_voi_jky_01_*`, `_02_*`, `_05_*`).
- Names are not case-sensitive. Several files for the same cue: `flac` > `wav` > `ogg` > `mp3`;
  the others are ignored (listed in `hachimi.log`).
- Any sample rate and channel count: stereo and mono are kept (more channels are folded to
  stereo, above 48 kHz is resampled to 48 kHz). Up to 15 minutes per file.

**Finding cue names**

- **Hachimi menu > Sound replace > Log played cues**: every sound the game plays is written to
  `<game>\hachimi\uma-sound-replace\cue_log.txt` (`cue sheet <tab> cue name`), and the last
  ones are shown in the menu (`*` = replaced). Play the scene, then look it up.
- `tools/list_cues.py` lists every cue of every downloaded sound file into a CSV
  (`pip install apsw-sqlite3mc`, then `python tools/list_cues.py --filter snd_bgm`).

**Hachimi menu, "Sound replace" section**

- **Use replacement sounds**: on/off (takes effect from the next sound that starts).
- The folder, how many files are ready / loading / broken, the first broken ones and why.
- **Reload files**: rescan after adding or changing files, without restarting the game.
  Files are also read at every game start. (Hidden on Hachimi builds without menu buttons.)
- **Log played cues** (see above).

Files are decoded in the background when the game starts. A file that is still being decoded
when the game plays it is decoded on the spot (a short pause the first time for long music).

## Settings

`<game>\hachimi\uma-sound-replace\settings.txt`, `key = value` lines:

| Key | Default | What |
|---|---|---|
| `enabled` | `true` | replacements on/off |
| `log_cues` | `false` | write played cues to `cue_log.txt` |
| `copy_categories` | `true` | a replaced sound gets the categories of the cue it replaces (volume settings) |
| `patch_length` | `true` | when the game asks how long a replaced cue is, it gets the file's length |
| `extra_voice_pool` | `true` | add a standard voice pool for replacements (see below) |

## How it works

- The game's audio library is CRI ADX2 (`cri_ware_unity.dll`). The plugin hooks its native
  player functions (Hachimi only hooks the managed wrappers): `criAtomExPlayer_SetCueId` /
  `SetCueName` / `SetCueIndex` remember each player's cue, and at `criAtomExPlayer_Start` /
  `Prepare` a cue with a ready replacement gets the file instead (`SetFormat` ADX + `SetData`).
  The original `Start` then runs on the game's own player.
- Files are decoded with [Symphonia](https://github.com/pdeljanov/Symphonia) and encoded to
  ADX (the library's own ADPCM format) once, kept in memory.
- Cue sheet names come from the ACB files the game loads (`criAtomExAcb_LoadAcbFile` /
  `LoadAcbData`, the file's `Name`). Sheets loaded before the plugin are matched by cue name
  only.
- Volume: the cue's categories (`criAtomExAcb_GetCueInfo*`) are put on the player for a
  replaced sound and taken off afterwards, unless the game sets categories on that player
  itself. Loops: a cue of endless length gets a looping file. Both read CRI's
  `CriAtomExCueInfo`, whose layout is checked on a real cue first (`hachimi.log` says
  `CriAtomExCueInfo layout verified` or `differs`; if it differs, replacements still play,
  without these).
- `extra_voice_pool`: one default standard (ADX/HCA) voice pool, made the first time a
  replacement plays, in case the game's pools only take other formats. Turn it off if
  replaced sounds misbehave.

## Troubleshooting

- **A replacement does not play**: turn on **Log played cues**, play the scene, and check the
  name in `cue_log.txt` (a `[replaced]` mark means the plugin did replace it). Check the menu
  for broken files. `hachimi.log` lists every problem with `[uma-sound-replace]`.
- **Silence instead of the replacement**: the game's player may have no voice for the format.
  Report the `hachimi.log` lines; `extra_voice_pool = true` is meant for this case.
- **The game misbehaves**: set `enabled = false` (menu or `settings.txt`), or remove the DLL
  from `load_libraries`.

## ภาษาไทย (สรุปสั้น)

1. build แล้ว copy `uma_sound_replace.dll` ไปที่ `<game>\hachimi\` และเพิ่มใน `load_libraries`
   ของ `config.json` (ดูขั้นตอนด้านบน)
2. วางไฟล์ `wav` / `mp3` / `flac` / `ogg` ในโฟลเดอร์ `<game>\hachimi\uma-sound-replace\sounds\`
   ตั้งชื่อไฟล์ตามชื่อ cue ที่ต้องการแทน เช่น `snd_voi_race_100100_0000.wav`
   (โฟลเดอร์ย่อยตั้งชื่ออะไรก็ได้ ยกเว้นโฟลเดอร์ที่ขึ้นต้นด้วย `snd_` จะหมายถึงแทนเฉพาะใน cue sheet นั้น)
3. ไม่รู้ชื่อ cue: เปิด **Log played cues** ในเมนู Hachimi เล่นฉากนั้น แล้วดูใน `cue_log.txt`
   หรือดูคอลัมน์ cue ในไฟล์ Excel รายชื่อเสียงพากย์
4. ไม่มีไฟล์ หรือไฟล์เสีย เกมจะเล่นเสียงเดิมของมันเอง เพิ่ม/แก้ไฟล์แล้วกด **Reload files** ได้เลยไม่ต้องปิดเกม
