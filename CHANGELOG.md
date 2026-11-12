# Changelog

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
