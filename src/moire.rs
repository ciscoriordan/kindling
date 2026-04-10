/// Moire/rainbow artifact correction for color e-ink screens.
///
/// Color e-ink displays (Kindle Colorsoft, Kobo Colour) use a CFA (color filter
/// array) overlay that can produce visible rainbow interference patterns - called
/// moire - when displaying high-frequency grayscale content like manga screentone.
///
/// This module provides a correction filter that suppresses moire artifacts while
/// preserving perceived sharpness:
///   1. A mild Gaussian blur (sigma ~1px) removes the high-frequency patterns
///      that trigger moire on the CFA grid.
///   2. An unsharp mask restores edge contrast lost in the blur step.
///
/// The filter should only be applied to grayscale source images destined for
/// color device profiles (colorsoft, fire-hd-10). Applying it to already-color
/// source images or grayscale-only devices is unnecessary.

use image::DynamicImage;

/// Remove moire/rainbow artifacts from a grayscale image intended for a color
/// e-ink display.
///
/// Applies a two-step process:
///   1. Gaussian blur with sigma ~1.0 to suppress high-frequency screentone
///      patterns that cause rainbow interference on CFA-based color e-ink.
///   2. Unsharp mask (threshold 1, sigma 0.5, amount ~80%) to restore edge
///      definition without reintroducing the problematic frequencies.
///
/// This function modifies the image in place. It is safe to call on any image,
/// but is only useful for grayscale content targeting color e-ink devices.
pub fn remove_moire(img: &mut DynamicImage) {
    // Step 1: Gaussian blur to suppress high-frequency moire patterns.
    // sigma=1.0 is mild enough to preserve detail while eliminating the
    // fine screentone that triggers CFA rainbow artifacts.
    let blurred = img.blur(1.0);

    // Step 2: Unsharp mask to recover edge sharpness.
    // The unsharpen method computes: original + amount * (original - blurred).
    // sigma=0.5 controls the blur radius for the mask itself.
    // threshold=1 avoids sharpening noise in flat areas.
    // A negative "amount" in the image crate's unsharpen is the sharpening
    // strength (the crate uses the convention where negative = sharpen).
    //
    // image 0.25 signature: unsharpen(sigma: f32, threshold: i32) -> DynamicImage
    // This applies: output = img + (img - blur(img, sigma)) where pixels differ
    // by more than threshold.
    *img = blurred.unsharpen(0.5, 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, RgbImage};

    #[test]
    fn test_remove_moire_grayscale() {
        // Create a small grayscale test image with a high-frequency checkerboard
        // pattern (simulates screentone that causes moire on color e-ink).
        let mut gray = GrayImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let val = if (x + y) % 2 == 0 { 200u8 } else { 50u8 };
                gray.put_pixel(x, y, image::Luma([val]));
            }
        }
        let mut img = DynamicImage::ImageLuma8(gray);

        // Verify the filter runs without error
        remove_moire(&mut img);

        // The output should still be a valid image of the same dimensions
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);

        // After moire correction, the extreme high-frequency pattern should be
        // smoothed out. Check that the pixel variance has decreased - the
        // checkerboard alternation between 50 and 200 should be reduced.
        let output_gray = img.to_luma8();
        let center = output_gray.get_pixel(32, 32)[0];
        let neighbor = output_gray.get_pixel(33, 32)[0];
        let original_diff: i32 = 150; // 200 - 50
        let filtered_diff = (center as i32 - neighbor as i32).abs();
        assert!(
            filtered_diff < original_diff,
            "Moire filter should reduce high-frequency contrast: original diff={}, filtered diff={}",
            original_diff,
            filtered_diff,
        );
        println!("  \u{2713} Grayscale moire: contrast diff {} -> {}", original_diff, filtered_diff);
    }

    #[test]
    fn test_remove_moire_rgb() {
        // The filter should also work on RGB images without panicking.
        let rgb = RgbImage::new(32, 32);
        let mut img = DynamicImage::ImageRgb8(rgb);
        remove_moire(&mut img);
        assert_eq!(img.width(), 32);
        assert_eq!(img.height(), 32);
        println!("  \u{2713} RGB 32x32 moire filter ran without error");
    }

    #[test]
    fn test_remove_moire_preserves_dimensions() {
        // Verify that various image sizes are handled correctly.
        for &(w, h) in &[(1, 1), (3, 3), (100, 200), (1072, 1448)] {
            let gray = GrayImage::new(w, h);
            let mut img = DynamicImage::ImageLuma8(gray);
            remove_moire(&mut img);
            assert_eq!(img.width(), w);
            assert_eq!(img.height(), h);
        }
        println!("  \u{2713} All 4 sizes preserved dimensions after moire filter");
    }
}
