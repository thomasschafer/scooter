use ratatui::style::Color;
use two_face::re_exports::syntect::highlighting::Color as SyntectColour;

// Finds the index (0-5) for an RGB component corresponding to the closest xterm cube level:
// 0, 95, 135, 175, 215 or 255.
#[allow(clippy::inline_always)]
#[inline(always)]
fn cube_idx(value: u8) -> u8 {
    if value < 48 {
        0
    } else if value < 115 {
        1
    } else if value < 155 {
        2
    } else if value < 195 {
        3
    } else if value < 235 {
        4
    } else {
        5
    }
}

/// Converts an RGB color to the closest xterm 256-color palette index.
/// This function prioritizes speed while maintaining reasonable accuracy.
///
/// The 256-color palette consists of:
/// - 0-15: Standard and high-intensity ANSI colors (not directly targeted here, but black and white are handled via cube/grayscale equivalents).
/// - 16-231: A 6x6x6 color cube.
/// - 232-255: A 24-step grayscale ramp.
fn to_256_colour(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 247 {
            return 231;
        }
        return 232 + ((r - 8) / 10);
    }

    16 + (cube_idx(r) * 36) + (cube_idx(g) * 6) + cube_idx(b)
}

pub fn to_ratatui_colour(colour: SyntectColour, true_colour: bool) -> Color {
    if true_colour {
        Color::Rgb(colour.r, colour.g, colour.b)
    } else {
        Color::Indexed(to_256_colour(colour.r, colour.g, colour.b))
    }
}

#[allow(clippy::identity_op, clippy::erasing_op)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_256_colour_black() {
        // Test black (0,0,0) should map to color 16
        assert_eq!(to_256_colour(0, 0, 0), 16);
        // Near-black values should also map to 16
        assert_eq!(to_256_colour(5, 5, 5), 16);
    }

    #[test]
    fn test_to_256_colour_white() {
        // Test white (255,255,255) should map to color 231 (end of color cube)
        assert_eq!(to_256_colour(255, 255, 255), 231);
        // Near-white values should also map to 231
        assert_eq!(to_256_colour(250, 250, 250), 231);
    }

    #[test]
    fn test_to_256_colour_grayscale() {
        // Test various grayscale values in between black and white
        // These should map to the grayscale ramp (232-255)
        assert_eq!(to_256_colour(8, 8, 8), 232);
        assert_eq!(to_256_colour(18, 18, 18), 233);
        assert_eq!(to_256_colour(28, 18, 28), 16 + (0 * 36) + (0 * 6) + 0); // Not grayscale, should go to cube
        assert_eq!(to_256_colour(128, 128, 128), 244);
        assert_eq!(to_256_colour(247, 247, 247), 255); // Last grayscale color
    }

    #[test]
    fn test_to_256_colour_primary_colors() {
        // Test primary colors (fully saturated R, G, B)
        assert_eq!(to_256_colour(255, 0, 0), 196); // Pure red is 16 + (5*36) + (0*6) + 0 = 196
        assert_eq!(to_256_colour(0, 255, 0), 46); // Pure green is 16 + (0*36) + (5*6) + 0 = 46
        assert_eq!(to_256_colour(0, 0, 255), 21); // Pure blue is 16 + (0*36) + (0*6) + 5 = 21
    }

    #[test]
    fn test_to_256_colour_secondary_colors() {
        // Test secondary colors
        assert_eq!(to_256_colour(255, 255, 0), 226); // Yellow is 16 + (5*36) + (5*6) + 0 = 226
        assert_eq!(to_256_colour(255, 0, 255), 201); // Magenta is 16 + (5*36) + (0*6) + 5 = 201
        assert_eq!(to_256_colour(0, 255, 255), 51); // Cyan is 16 + (0*36) + (5*6) + 5 = 51
    }

    #[test]
    fn test_to_256_colour_color_thresholds() {
        // Test color threshold boundaries for the cube_idx function

        // Just below and above the first threshold (48)
        assert_eq!(cube_idx(47), 0);
        assert_eq!(cube_idx(48), 1);

        // Just below and above the second threshold (115)
        assert_eq!(cube_idx(114), 1);
        assert_eq!(cube_idx(115), 2);

        // Just below and above the third threshold (155)
        assert_eq!(cube_idx(154), 2);
        assert_eq!(cube_idx(155), 3);

        // Just below and above the fourth threshold (195)
        assert_eq!(cube_idx(194), 3);
        assert_eq!(cube_idx(195), 4);

        // Just below and above the fifth threshold (235)
        assert_eq!(cube_idx(234), 4);
        assert_eq!(cube_idx(235), 5);
    }

    #[test]
    fn test_specific_xterm_values() {
        // The 6 specific red values in the cube
        assert_eq!(to_256_colour(0, 0, 0), 16); // Black (also grayscale)
        assert_eq!(to_256_colour(95, 0, 0), 52); // Dark red
        assert_eq!(to_256_colour(135, 0, 0), 88); // Medium-dark red
        assert_eq!(to_256_colour(175, 0, 0), 124); // Medium red
        assert_eq!(to_256_colour(215, 0, 0), 160); // Medium-bright red
        assert_eq!(to_256_colour(255, 0, 0), 196); // Bright red

        // A few exact color cube combinations
        assert_eq!(to_256_colour(95, 135, 175), 16 + (1 * 36) + (2 * 6) + 3);
        assert_eq!(to_256_colour(215, 95, 135), 16 + (4 * 36) + (1 * 6) + 2);
    }
}
