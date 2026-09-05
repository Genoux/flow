# assets

`Inter-Medium.ttf` is the font the island's toast is drawn with. It is Inter
4.1 Medium subset to Latin-1 plus the punctuation the messages use, which is
what takes it from 417 KB to 16 KB - the daemon renders a handful of fixed
English strings and has no use for the rest.

Vendored rather than loaded from the system so the toast looks the same on
every machine and needs no fontconfig in a daemon that otherwise has no UI
stack at all. Licensed under the SIL Open Font License 1.1, see
`Inter-LICENSE.txt`.

Regenerate with:

    pyftsubset Inter-Medium.ttf \
      --unicodes="U+0020-007E,U+00A0-00FF,U+2018-201D,U+2026,U+2013,U+2014" \
      --layout-features='' --no-hinting --desubroutinize \
      --output-file=assets/Inter-Medium.ttf

---

`island-show.wav` and `island-hide.wav` are the sounds the island arrives and
leaves on: the `select` and `deselect` cues from the **minimal** pack of
[uisfx](https://uisfx.com) 0.4.0, 0.23s each. Minimal because the island is a
quiet indicator and the loud packs argue with it.

Decoded to 48kHz mono 16-bit WAV rather than shipped as the original MP3 so
`paplay` can take them straight from a pipe - it reads whatever libsndfile
reads, and libsndfile does not read MP3. 20 KB each, embedded in the binary
next to the font.

The audio is CC0 1.0 (public domain), so no attribution is required; this note
is provenance, not a licence obligation.

Regenerate with:

    npm pack uisfx && tar xzf uisfx-*.tgz
    ffmpeg -i package/sounds/minimal/select.mp3 \
      -ar 48000 -ac 1 -c:a pcm_s16le assets/island-show.wav
    ffmpeg -i package/sounds/minimal/deselect.mp3 \
      -ar 48000 -ac 1 -c:a pcm_s16le assets/island-hide.wav

---

`style-woodland.jpg` is the first frame of the user-provided clip
`DTS_MISC_(JOEY_VIDEOS)__Joey_Bania_Clips_ID618.mp4`, found in Downloads.
The source filename credits Joey Bania. Its mossy woodland close-up supplies
the Style banner; the UI crops it at display time and overlays a dark gradient
for text contrast. The original video remains in Downloads.

Extract the same frame with:

    ffmpeg -i 'DTS_MISC_(JOEY_VIDEOS)__Joey_Bania_Clips_ID618.mp4' \
      -frames:v 1 -q:v 2 assets/style-woodland.jpg

`vocabulary-nick-fancher.jpg` is a byte-for-byte project copy of the
user-selected `DTS_Misc_1_(Nick_Fancher)_Nick_Fancher_Photos_ID5032.jpg`
from Downloads. The source filename credits Nick Fancher. The Vocabulary
banner frames the upper portion at display time to keep the subject's head
visible in a wide crop. All banner images are embedded using `include_bytes!`;
the application never reads them from Downloads at runtime.

`NotoSerifDisplay-Regular.ttf` supplies the editorial headline face. The font
is vendored so the banner's metrics and appearance are consistent across
machines. Copyright 2022 The Noto Project Authors; licensed under the SIL
Open Font License 1.1, included in `NotoSerifDisplay-LICENSE.txt`.

See [the brand direction](../design/brand-direction.md) for the visual rationale.

`overview-mouthwash.jpg` is the first frame of the user-selected
`DTS_Micro_Mouthwash_Studios_Clips_ID263.mp4` from Downloads. The source
filename credits Mouthwash Studios. The extracted frame is embedded in the
Overview banner; the original clip remains in Downloads.

Extract the same frame with:

    ffmpeg -i DTS_Micro_Mouthwash_Studios_Clips_ID263.mp4 \
      -frames:v 1 -q:v 2 assets/overview-mouthwash.jpg

`overview-mouthwash-wide.jpg` is an AI-extended variant of the original
Mouthwash frame, made with the built-in imagegen tool for the 180 px Overview
banner. The original extraction is retained above. The edit prompt asks to
zoom out, preserve the organic subject, olive/teal palette and film grain,
extend dark surroundings on the left for copy, and add no text or frame.
