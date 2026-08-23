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
