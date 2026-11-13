# Changelog

## 0.1.1 (13th November 2026)

- **Copies decoded many lines out of place are now lined up.** A decoder that misses a picture's header and starts from the line sync pulses can misjudge which line it joined at, by tens of lines.
  - The search up and down now reaches a fifth of the picture's height (48 lines on a 240-line picture), not 6 lines.
  - Before, such a copy was used at the wrong height, and a band of the picture could come from the wrong place.
- **A copy that can't be lined up is left out,** and its record says so. That's a copy whose best match is at the edge of the search, or unclear and needing a real move. `Combined` has a new field, `left_out`.
- **Pictures made by 0.1.0 are made again once** by the command line (`index.json` entries now carry a method number).
  - A picture that comes out the same is left as it is.
  - One that changes replaces the old file, which moves to `Replaced/`.

## 0.1.0 (12th November 2026)

- First release.
- Finds which pictures are copies of the same one, and combines them with nothing made up:
  - copies are lined up;
  - swapped Robot 36 colours are put back;
  - each line is weighted by its noise;
  - lines no copy received are shown hatched grey.
- Reads PNG, JPEG and BMP. It keeps the strongest copy's details (PNG text, and JPEG EXIF and comment), and adds four fields saying how the picture was combined.
- Copies saved at double (or another whole multiple of) the size are averaged down and combined.
- A record of how each picture was made.
- The command line `sstv-stacker`: running it again rebuilds only what has new copies, and keeps replaced files in `Replaced/`.
- A web page that runs in the browser, with nothing uploaded.
