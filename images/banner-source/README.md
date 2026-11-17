# Banner source

Both banners use `banner.svg`. It clips the illustration from the original
banner, places the rasterized wordmark from `wordmark.png`, and draws the
tagline in [Literata](https://github.com/googlefonts/literata) (weight 420,
optical size 24, SIL Open Font License 1.1).

The wordmark uses [Nitti Grotesk Medium](https://boldmonday.com/typefaces/nitti-grotesk/),
designed by Pieter van Rosmalen and published by Bold Monday. It is set in
title case at 163 px with −2 px letter spacing in the 1536 × 768 layout.
The source PNG is rendered at three times its placement size. Neither font
file is included or needed to regenerate the banners.

From the repository root, regenerate both images with `rsvg-convert` and
ImageMagick:

```sh
rsvg-convert images/banner-source/banner.svg -o /tmp/kindling-banner.png
magick /tmp/kindling-banner.png -background white -alpha remove -strip \
  -sampling-factor 4:4:4 -quality 96 images/kindling_social.jpg
rsvg-convert --width 1280 --height 640 images/banner-source/banner.svg \
  -o .github/social-preview.png
```

To publish the 1280 × 640 PNG as the GitHub social preview, upload it under
**Settings → General → Social preview**.
