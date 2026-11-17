# Banner source

Both banners use `banner.svg`. It clips the illustration from the original
banner and sets the name in [Space Grotesk](https://floriankarsten.github.io/space-grotesk/)
(weight 620) and the tagline in [Literata](https://github.com/googlefonts/literata)
(weight 420, optical size 24). Both fonts use the SIL Open Font License 1.1.
The SVG includes outlined lettering, so rendering does not require the fonts
to be installed.

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
