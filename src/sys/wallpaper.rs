//! macOS generated color wallpapers have no image URL (AppKit returns DefaultDesktop instead).
use std::hash::{DefaultHasher, Hash, Hasher};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{
    CFArray, CFPropertyList, CFRetained, CFString, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorRenderingIntent,
    CGColorSpace, CGContext, CGGradient, CGGradientDrawingOptions, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNumber, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString,
};

fn plist(data: &NSData) -> Option<Retained<AnyObject>> {
    // Immutable property lists; no format output is needed.
    unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            data,
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
        .ok()
    }
}

fn get(object: &AnyObject, path: &[&str]) -> Option<Retained<AnyObject>> {
    let (key, rest) = path.split_first()?;
    let dictionary = object.downcast_ref::<NSDictionary>()?;
    let value = dictionary.objectForKey(&NSString::from_str(key))?;
    if rest.is_empty() {
        Some(value)
    } else {
        get(&value, rest)
    }
}

fn desktop_content(store: &AnyObject, display: &str) -> Option<Retained<AnyObject>> {
    // shortcut: Space-specific wallpaper overrides are not resolved here. Do not substitute a
    // global color when such overrides exist; add Space UUID resolution before supporting them.
    if let Some(spaces) = get(store, &["Spaces"])
        && spaces.downcast_ref::<NSDictionary>()?.count() != 0
    {
        return None;
    }
    get(store, &["Displays", display, "Desktop", "Content"])
        .or_else(|| get(store, &["AllSpacesAndDisplays", "Desktop", "Content"]))
        .or_else(|| get(store, &["SystemDefault", "Desktop", "Content"]))
}

fn wallpaper_color(content: &AnyObject) -> Option<(CFRetained<CGColor>, bool, u64)> {
    let choices = get(content, &["Choices"])?;
    let choices = choices.downcast_ref::<NSArray<AnyObject>>()?;
    if choices.count() != 1 {
        return None;
    }
    let choice = choices.firstObject()?;
    let provider = get(&choice, &["Provider"])?;
    if provider.downcast_ref::<NSString>()?.to_string() != "com.apple.wallpaper.choice.color" {
        return None;
    }
    let options = get(content, &["EncodedOptionValues"])?;
    let options = options.downcast_ref::<NSData>()?;
    let values = plist(options)?;
    let enabled = get(&values, &[
        "values",
        "shouldGenerateGradient",
        "toggle",
        "_0",
        "isOn",
    ]);
    let gradient = match enabled {
        Some(value) => value.downcast_ref::<NSNumber>()?.boolValue(),
        None => false,
    };
    // Previously, solid colors were rejected here when the gradient toggle was off.
    // if !enabled.downcast_ref::<NSNumber>()?.boolValue() { return None; }
    let configuration = get(&choice, &["Configuration"])?;
    let configuration = configuration.downcast_ref::<NSData>()?;
    let selection = plist(configuration)?;
    let kind = get(&selection, &["type"])?;
    let color = match kind.downcast_ref::<NSString>()?.to_string().as_str() {
        "customColor" => custom_color(&values)?,
        "systemColor" => {
            let preset = get(&selection, &["systemColor"])?;
            let preset = preset.downcast_ref::<NSDictionary>()?;
            if preset.count() != 1 {
                return None;
            }
            let name = preset.allKeys().firstObject()?;
            preset_color(&name.downcast_ref::<NSString>()?.to_string())?
        }
        _ => return None,
    };
    let mut hash = DefaultHasher::new();
    options.to_vec().hash(&mut hash);
    configuration.to_vec().hash(&mut hash);
    Some((color, gradient, hash.finish()))
}

fn custom_color(values: &AnyObject) -> Option<CFRetained<CGColor>> {
    let color = get(values, &["values", "customColor", "color", "_0", "color"])?;
    let components = get(&color, &["components"])?;
    let components = components.downcast_ref::<NSArray<AnyObject>>()?;
    if components.count() != 4 {
        return None;
    }
    let mut rgba = [0.0; 4];
    for (index, value) in rgba.iter_mut().enumerate() {
        // Check each element's runtime type before reading untrusted plist data.
        let number = components.objectAtIndex(index);
        *value = number.downcast_ref::<NSNumber>()?.doubleValue();
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return None;
        }
    }
    let space = get(&color, &["colorSpace"])?;
    let space = plist(space.downcast_ref::<NSData>()?)?;
    // Foundation property-list objects are toll-free bridged to CFPropertyList.
    let space = unsafe {
        CGColorSpace::with_property_list(&*(&*space as *const AnyObject as *const CFPropertyList))?
    };
    if CGColorSpace::number_of_components(Some(&space)) != 3 {
        return None;
    }
    // Four validated components for an RGB color space, including alpha.
    unsafe { CGColor::new(Some(&space), rgba.as_ptr()) }
}

fn preset_color(name: &str) -> Option<CFRetained<CGColor>> {
    // sRGB values from macOS 26 WallpaperGradientExtension's BuiltInColors palette.
    let [r, g, b] = match name {
        "black" => [0, 0, 0],
        "blueViolet" => [104, 103, 175],
        "cyan" => [20, 175, 208],
        "dustyRose" => [224, 110, 121],
        "electricBlue" => [65, 84, 214],
        "gold2" => [252, 219, 202],
        "gold" => [244, 222, 201],
        "ocher" => [212, 166, 87],
        "plum" => [205, 75, 147],
        "redOrange" => [234, 62, 35],
        "roseGold" => [246, 211, 206],
        "silver" => [227, 228, 229],
        "softPink" => [252, 221, 229],
        "spaceGrayPro" => [122, 123, 128],
        "spaceGray" => [190, 191, 197],
        "stone" => [84, 85, 84],
        "teal" => [0, 121, 116],
        "turquoiseGreen" => [106, 196, 162],
        "yellow" => [253, 184, 19],
        _ => return None,
    };
    Some(CGColor::new_srgb(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        1.0,
    ))
}

/// The color adjustment used by macOS 26's WallpaperGradientExtension: increase saturation,
/// then lighten dark/mid colors or darken very bright colors. Bright colors reverse the stops.
fn adjusted_color([r, g, b, alpha]: [f64; 4]) -> ([f64; 4], bool) {
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let amount = if luminance <= 0.25 {
        0.2
    } else if luminance <= 0.5 {
        0.25
    } else if luminance <= 0.75 {
        0.3
    } else {
        -0.1
    };
    let adjust = |c: f64| {
        let saturated = c + (c - luminance) * 0.2;
        (saturated
            + amount
                * if amount < 0.0 {
                    saturated
                } else {
                    1.0 - saturated
                })
        .clamp(0.0, 1.0)
    };
    ([adjust(r), adjust(g), adjust(b), alpha], amount < 0.0)
}

fn render(color: &CGColor, generate_gradient: bool) -> Option<CFRetained<CGImage>> {
    let srgb = CGColorSpace::with_name(Some(&CFString::from_str("kCGColorSpaceSRGB")))?;
    // Convert before applying Apple's luminance calculation, including for Generic RGB colors.
    let base = unsafe {
        CGColor::new_copy_by_matching_to_color_space(
            Some(&srgb),
            CGColorRenderingIntent::RenderingIntentDefault,
            Some(color),
            None,
        )?
    };
    if CGColor::number_of_components(Some(&base)) != 4 {
        return None;
    }
    // A vertical strip stretches horizontally without changing the gradient. CoreGraphics owns
    // the buffer; the resulting CGImage participates in Overview's existing background cache.
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            1,
            1024,
            8,
            4,
            Some(&srgb),
            CGImageAlphaInfo::PremultipliedLast.0,
        )?
    };
    let bounds = CGRect::new(CGPoint::ZERO, CGSize::new(1.0, 1024.0));
    // macOS composites translucent wallpaper colors over white, not application windows.
    CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
    CGContext::fill_rect(Some(&context), bounds);
    if generate_gradient {
        // The converted RGB color owns exactly four components for its lifetime.
        let rgba = unsafe { std::slice::from_raw_parts(CGColor::components(Some(&base)), 4) };
        let ([r, g, b, a], reverse) = adjusted_color(rgba.try_into().ok()?);
        // Apple's derived stop uses Generic RGB, while the original stop remains sRGB.
        let adjusted = CGColor::new_generic_rgb(r, g, b, a);
        let colors = if reverse {
            [&*adjusted, &*base]
        } else {
            [&*base, &*adjusted]
        };
        let colors = CFArray::from_objects(&colors);
        // Evenly spaced, correctly typed CGColor stops retained by the array.
        let gradient = unsafe {
            CGGradient::with_colors(Some(&srgb), Some(colors.as_opaque()), std::ptr::null())?
        };
        CGContext::draw_linear_gradient(
            Some(&context),
            Some(&gradient),
            CGPoint::ZERO,
            CGPoint::new(0.0, 1024.0),
            CGGradientDrawingOptions::empty(),
        );
    } else {
        CGContext::set_fill_color_with_color(Some(&context), Some(&base));
        CGContext::fill_rect(Some(&context), bounds);
    }
    CGBitmapContextCreateImage(Some(&context))
}

pub(crate) fn color_for_display(display: &str) -> Option<(String, CFRetained<CGImage>)> {
    let path =
        dirs::home_dir()?.join("Library/Application Support/com.apple.wallpaper/Store/Index.plist");
    let store = plist(&NSData::with_bytes(&std::fs::read(path).ok()?))?;
    let content = desktop_content(&store, display)?;
    let (color, gradient, hash) = wallpaper_color(&content)?;
    Some((format!("color:{hash:x}"), render(&color, gradient)?))
}

#[cfg(test)]
mod tests {
    use objc2_foundation::{NSDataBase64EncodingOptions, NSPropertyListFormat};

    use super::*;

    fn xml(body: &str) -> Retained<AnyObject> {
        plist(&NSData::with_bytes(
            format!("<plist version=\"1.0\">{body}</plist>").as_bytes(),
        ))
        .unwrap()
    }

    fn content(provider: &str, enabled: bool, components: &str) -> String {
        configured_content(
            provider,
            enabled,
            components,
            "<key>type</key><string>customColor</string>",
        )
    }

    fn configured_content(
        provider: &str,
        enabled: bool,
        components: &str,
        selection: &str,
    ) -> String {
        // Synthetic fixture in Apple's nested binary-plist format, including the color space.
        let values = xml(&format!(
            r#"<dict><key>values</key><dict>
            <key>shouldGenerateGradient</key><dict><key>toggle</key><dict><key>_0</key><dict>
            <key>isOn</key><{enabled}/></dict></dict></dict>
            <key>customColor</key><dict><key>color</key><dict><key>_0</key><dict><key>color</key><dict>
            <key>components</key><array>{components}</array><key>colorSpace</key>
            <data>YnBsaXN0MDBfEBdrQ0dDb2xvclNwYWNlR2VuZXJpY1JHQggAAAAAAAABAQAAAAAAAAABAAAAAAAAAAAAAAAAAAAAIg==</data>
            </dict></dict></dict></dict></dict></dict>"#
        ));
        let data = unsafe {
            NSPropertyListSerialization::dataWithPropertyList_format_options_error(
                &values,
                NSPropertyListFormat::BinaryFormat_v1_0,
                0,
            )
            .unwrap()
        };
        let encoded = data.base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty());
        let configuration = NSData::with_bytes(
            format!("<plist version=\"1.0\"><dict>{selection}</dict></plist>").as_bytes(),
        )
        .base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty());
        format!(
            r#"<dict><key>Choices</key><array><dict><key>Provider</key><string>{provider}</string>
            <key>Configuration</key><data>{configuration}</data>
            </dict></array><key>EncodedOptionValues</key><data>{encoded}</data></dict>"#
        )
    }

    const COLOR: &str = "com.apple.wallpaper.choice.color";
    const RGBA: &str = "<real>0.15</real><real>0.08</real><real>0.06</real><real>1</real>";

    #[test]
    fn reads_nested_gradient_and_rejects_other_or_malformed_wallpapers() {
        let good = content(COLOR, true, RGBA);
        let (color, gradient, key) = wallpaper_color(&xml(&good)).unwrap();
        assert_eq!(CGColor::number_of_components(Some(&color)), 4);
        assert!(gradient);
        let changed = content(COLOR, true, &RGBA.replace("0.15", "0.25"));
        assert_ne!(key, wallpaper_color(&xml(&changed)).unwrap().2);
        for bad in [
            content("com.apple.wallpaper.choice.image", true, RGBA),
            content(COLOR, true, "<real>0.5</real>"),
            content(
                COLOR,
                true,
                &RGBA.replace("<real>0.15</real>", "<string>red</string>"),
            ),
            content(COLOR, true, &RGBA.replace("0.15", "NaN")),
            content(COLOR, true, &RGBA.replace("0.15", "2.0")),
            good.replace("<data>", "<string>").replace("</data>", "</string>"),
        ] {
            assert!(wallpaper_color(&xml(&bad)).is_none());
        }
        assert!(plist(&NSData::with_bytes(b"not a plist")).is_none());
    }

    #[test]
    fn display_image_override_does_not_inherit_global_gradient() {
        let global = content(COLOR, true, RGBA);
        let image = content("com.apple.wallpaper.choice.image", true, RGBA);
        let store = xml(&format!(
            r#"<dict>
            <key>AllSpacesAndDisplays</key><dict><key>Desktop</key><dict><key>Content</key>{global}</dict></dict>
            <key>Displays</key><dict><key>external</key><dict><key>Desktop</key><dict><key>Content</key>{image}</dict></dict></dict>
            <key>Spaces</key><dict/></dict>"#
        ));
        assert!(wallpaper_color(&desktop_content(&store, "builtin").unwrap()).is_some());
        assert!(wallpaper_color(&desktop_content(&store, "external").unwrap()).is_none());
        let ambiguous = xml("<dict><key>Spaces</key><dict><key>space</key><dict/></dict></dict>");
        assert!(desktop_content(&ambiguous, "builtin").is_none());
    }

    #[test]
    fn native_gradient_adjustment_handles_all_brightness_bands() {
        for (gray, expected, reversed) in [
            (0.1, 0.28, false),
            (0.4, 0.55, false),
            (0.6, 0.72, false),
            (0.9, 0.81, true),
        ] {
            let (color, reverse) = adjusted_color([gray, gray, gray, 0.8]);
            assert_eq!(reverse, reversed);
            for component in &color[..3] {
                assert!((component - expected).abs() < 1e-12);
            }
            assert_eq!(color[3], 0.8);
        }
        let (red, _) = adjusted_color([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(red[0], 1.0);
        assert!((red[1] - 0.165984).abs() < 1e-12);
    }

    #[test]
    fn preset_selection_overrides_stale_custom_color_and_invalidates_cache() {
        let preset = |name: &str| {
            configured_content(
                COLOR,
                false,
                "<string>stale</string>",
                &format!(
                    "<key>type</key><string>systemColor</string><key>systemColor</key><dict><key>{name}</key><dict/></dict>"
                ),
            )
        };
        let (color, gradient, blue_key) = wallpaper_color(&xml(&preset("blueViolet"))).unwrap();
        assert!(!gradient);
        let image = render(&color, gradient).unwrap();
        let provider = CGImage::data_provider(Some(&image)).unwrap();
        let pixels = objc2_core_graphics::CGDataProvider::data(Some(&provider)).unwrap().to_vec();
        assert_eq!(&pixels[..4], &[104, 103, 175, 255]);
        let (_, _, cyan_key) = wallpaper_color(&xml(&preset("cyan"))).unwrap();
        assert_ne!(blue_key, cyan_key, "preset selection is part of the cache key");
        assert!(wallpaper_color(&xml(&preset("unknown"))).is_none());
    }

    #[test]
    fn all_native_presets_render_as_solid_colors_and_gradients() {
        for name in [
            "black",
            "blueViolet",
            "cyan",
            "dustyRose",
            "electricBlue",
            "gold2",
            "gold",
            "ocher",
            "plum",
            "redOrange",
            "roseGold",
            "silver",
            "softPink",
            "spaceGrayPro",
            "spaceGray",
            "stone",
            "teal",
            "turquoiseGreen",
            "yellow",
        ] {
            for gradient in [false, true] {
                let selection = format!(
                    "<key>type</key><string>systemColor</string><key>systemColor</key><dict><key>{name}</key><dict/></dict>"
                );
                let fixture = configured_content(COLOR, gradient, RGBA, &selection);
                let (color, enabled, _) = wallpaper_color(&xml(&fixture)).unwrap();
                assert_eq!(enabled, gradient);
                let image = render(&color, enabled).unwrap();
                let provider = CGImage::data_provider(Some(&image)).unwrap();
                let pixels =
                    objc2_core_graphics::CGDataProvider::data(Some(&provider)).unwrap().to_vec();
                assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
            }
        }
    }

    #[test]
    fn solid_colors_are_recognized_and_have_distinct_cache_keys() {
        let (_, gradient, gradient_key) =
            wallpaper_color(&xml(&content(COLOR, true, RGBA))).unwrap();
        let (_, solid, solid_key) = wallpaper_color(&xml(&content(COLOR, false, RGBA))).unwrap();
        assert!(gradient);
        assert!(!solid);
        assert_ne!(gradient_key, solid_key);
    }

    #[test]
    fn solid_and_gradient_opacity_is_flattened_over_white() {
        for gradient in [false, true] {
            let mut opaque = Vec::new();
            for alpha in [1.0, 0.5, 0.0] {
                let rgba = RGBA.replace("<real>1</real>", &format!("<real>{alpha}</real>"));
                let (color, enabled, _) =
                    wallpaper_color(&xml(&content(COLOR, gradient, &rgba))).unwrap();
                let image = render(&color, enabled).unwrap();
                let provider = CGImage::data_provider(Some(&image)).unwrap();
                let pixels =
                    objc2_core_graphics::CGDataProvider::data(Some(&provider)).unwrap().to_vec();
                assert!(
                    pixels.chunks_exact(4).all(|pixel| pixel[3] == 255),
                    "wallpaper must never reveal windows behind the overview"
                );
                if !gradient {
                    assert!(
                        pixels.chunks_exact(4).all(|pixel| pixel == &pixels[..4]),
                        "solid wallpaper must be uniform"
                    );
                }
                if alpha == 1.0 {
                    opaque = pixels;
                } else {
                    for (pixel, original) in pixels.chunks_exact(4).zip(opaque.chunks_exact(4)) {
                        for channel in 0..3 {
                            let expected = original[channel] as f64 * alpha + 255.0 * (1.0 - alpha);
                            assert!(
                                (pixel[channel] as f64 - expected).abs() <= 1.0,
                                "opacity must blend over white, including at zero opacity"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn renders_color_managed_gradient_image() {
        let (color, _, _) = wallpaper_color(&xml(&content(COLOR, true, RGBA))).unwrap();
        let image = render(&color, true).unwrap();
        assert_eq!(CGImage::width(Some(&image)), 1);
        assert_eq!(CGImage::height(Some(&image)), 1024);
        let provider = CGImage::data_provider(Some(&image)).unwrap();
        let pixels = objc2_core_graphics::CGDataProvider::data(Some(&provider)).unwrap().to_vec();
        let stride = CGImage::bytes_per_row(Some(&image));
        let first = &pixels[..4];
        let last = &pixels[1023 * stride..1023 * stride + 4];
        assert_eq!(first[3], 255);
        assert_eq!(last[3], 255);
        assert!(
            first[0] > last[0],
            "the image top should be lighter than its bottom"
        );
        assert!(
            first[0] > first[1] && first[1] > first[2],
            "preserve the warm base color"
        );
    }
}
