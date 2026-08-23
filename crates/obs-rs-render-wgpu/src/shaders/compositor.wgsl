@group(0) @binding(0) var layer_texture: texture_2d<f32>;
@group(0) @binding(1) var background_texture: texture_2d<f32>;
struct Parameters { values: array<i32> };
@group(0) @binding(2) var<storage, read> parameters: Parameters;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    var output: VertexOutput;
    let position = positions[vertex];
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = vec2<f32>((position.x + 1.0) * 0.5, (1.0 - position.y) * 0.5);
    return output;
}

fn chroma_nonlinear_channel(value: f32) -> f32 {
    if (value <= 0.0031308) {
        return 12.92 * value;
    }
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

fn chroma_components(color: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(
        -0.100644 * color.r - 0.338572 * color.g + 0.439216 * color.b + 0.501961,
        0.439216 * color.r - 0.398942 * color.g - 0.040274 * color.b + 0.501961,
    );
}

fn chroma_key_mask(base: f32, width: f32) -> f32 {
    if (width <= 0.0) {
        if (base > 0.0) {
            return 1.0;
        }
        return 0.0;
    }
    return pow(clamp(base / width, 0.0, 1.0), 1.5);
}

fn layer_pixel(position: vec2<i32>) -> vec4<i32> {
    let x = position.x;
    let y = position.y;
    let target_width = parameters.values[0];
    let target_height = parameters.values[1];
    let source_width = parameters.values[2];
    let source_height = parameters.values[3];
    // Scene transforms are expressed in canvas pixels. Map the viewport
    // fragment back into that canvas before applying the source transform.
    let canvas_x = x * source_width / target_width;
    let canvas_y = y * source_height / target_height;
    let local_x = canvas_x - parameters.values[6];
    let local_y = canvas_y - parameters.values[7];
    if (local_x < 0 || local_y < 0) {
        return vec4<i32>(0);
    }
    let crop_left = parameters.values[11];
    let crop_top = parameters.values[12];
    let visible_right = source_width - parameters.values[13];
    let visible_bottom = source_height - parameters.values[14];
    var source_x: i32;
    var source_y: i32;
    if (parameters.values[15] == 0) {
        source_x = crop_left + local_x * 1000 / parameters.values[4];
        source_y = crop_top + local_y * 1000 / parameters.values[5];
    } else {
        // Rotation is around the centre of the visible, scaled source. The
        // inverse matrix maps a target pixel back into source space, matching
        // the CPU reference transform's screen-coordinate convention.
        let visible_width = visible_right - crop_left;
        let visible_height = visible_bottom - crop_top;
        let scaled_width = f32(visible_width) * f32(parameters.values[4]) / 1000.0;
        let scaled_height = f32(visible_height) * f32(parameters.values[5]) / 1000.0;
        let center_x = f32(parameters.values[6]) + scaled_width / 2.0;
        let center_y = f32(parameters.values[7]) + scaled_height / 2.0;
        let angle = f32(parameters.values[15]) * 3.14159265359 / 180000.0;
        let sine = sin(angle);
        let cosine = cos(angle);
        let delta_x = f32(canvas_x) + 0.5 - center_x;
        let delta_y = f32(canvas_y) + 0.5 - center_y;
        let transformed_x = cosine * delta_x + sine * delta_y + scaled_width / 2.0;
        let transformed_y = -sine * delta_x + cosine * delta_y + scaled_height / 2.0;
        if (transformed_x < 0.0 || transformed_y < 0.0 ||
            transformed_x >= scaled_width || transformed_y >= scaled_height) {
            return vec4<i32>(0);
        }
        source_x = crop_left + i32(floor(transformed_x * 1000.0 /
            f32(parameters.values[4])));
        source_y = crop_top + i32(floor(transformed_y * 1000.0 /
            f32(parameters.values[5])));
    }
    if (source_x < crop_left || source_x >= visible_right ||
        source_y < crop_top || source_y >= visible_bottom) {
        return vec4<i32>(0);
    }
    if (parameters.values[8] != 0) {
        source_x = crop_left + visible_right - 1 - source_x;
    }
    if (parameters.values[9] != 0) {
        source_y = crop_top + visible_bottom - 1 - source_y;
    }
    let sampled = textureLoad(layer_texture, vec2<i32>(source_x, source_y), 0);
    var pixel = vec4<i32>(floor(sampled * 255.0 + vec4<f32>(0.5)));
    pixel.a = pixel.a * parameters.values[10] / 255;
    let filter_count = parameters.values[16];
    var filter_index = 0;
    loop {
        if (filter_index >= filter_count) { break; }
        let filter_offset = 17 + filter_index * 7;
        let kind = parameters.values[filter_offset];
        let value = parameters.values[filter_offset + 1];
        if (kind == 0) {
            let luma = (pixel.r * 77 + pixel.g * 150 + pixel.b * 29) / 256;
            pixel.r = luma;
            pixel.g = luma;
            pixel.b = luma;
        } else if (kind == 1) {
            let multiplier = value + 1000;
            pixel.r = clamp(pixel.r * multiplier / 1000, 0, 255);
            pixel.g = clamp(pixel.g * multiplier / 1000, 0, 255);
            pixel.b = clamp(pixel.b * multiplier / 1000, 0, 255);
        } else if (kind == 2) {
            pixel.a = pixel.a * value / 255;
        } else if (kind == 3) {
            let crop_left = parameters.values[filter_offset + 1];
            let crop_top = parameters.values[filter_offset + 2];
            let crop_right = parameters.values[filter_offset + 3];
            let crop_bottom = parameters.values[filter_offset + 4];
            let width = parameters.values[0];
            let height = parameters.values[1];
            if (position.x < crop_left || position.x >= width - crop_right ||
                position.y < crop_top || position.y >= height - crop_bottom) {
                pixel = vec4<i32>(0);
            }
        } else if (kind == 4) {
            let gamma = f32(parameters.values[filter_offset + 1]) / 1000.0;
            var gamma_exponent: f32;
            if (gamma < 0.0) {
                gamma_exponent = -gamma + 1.0;
            } else {
                gamma_exponent = 1.0 / (gamma + 1.0);
            }
            var color = vec3<f32>(f32(pixel.r), f32(pixel.g), f32(pixel.b)) / 255.0;
            color = pow(color, vec3<f32>(gamma_exponent));

            let contrast_value = f32(parameters.values[filter_offset + 2]) / 1000.0;
            let contrast = select(
                contrast_value + 1.0,
                1.0 / (-contrast_value + 1.0),
                contrast_value < 0.0,
            );
            let brightness = f32(parameters.values[filter_offset + 3]) / 1000.0;
            color = color * contrast + vec3<f32>(brightness);

            let saturation = f32(parameters.values[filter_offset + 4]) / 1000.0 + 1.0;
            let luma = dot(color, vec3<f32>(0.299, 0.587, 0.114));
            color = vec3<f32>(luma) + saturation * (color - vec3<f32>(luma));

            let half_angle = f32(parameters.values[filter_offset + 5]) *
                3.14159265359 / 360.0;
            let quaternion_axis = sin(half_angle) / sqrt(3.0);
            let square = quaternion_axis * quaternion_axis;
            let diagonal = 0.5 - 2.0 * square;
            let a_line = square + quaternion_axis * cos(half_angle);
            let b_line = square - quaternion_axis * cos(half_angle);
            color = vec3<f32>(
                2.0 * (diagonal * color.r + b_line * color.g + a_line * color.b),
                2.0 * (a_line * color.r + diagonal * color.g + b_line * color.b),
                2.0 * (b_line * color.r + a_line * color.g + diagonal * color.b),
            );
            color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
            pixel.r = i32(floor(color.r * 255.0 + 0.5));
            pixel.g = i32(floor(color.g * 255.0 + 0.5));
            pixel.b = i32(floor(color.b * 255.0 + 0.5));
            let opacity = parameters.values[filter_offset + 6];
            pixel.a = i32(floor(f32(pixel.a) * f32(opacity) / 1000.0 + 0.5));
        } else if (kind == 5) {
            let key = vec3<f32>(
                f32(parameters.values[filter_offset + 1]),
                f32(parameters.values[filter_offset + 2]),
                f32(parameters.values[filter_offset + 3]),
            ) / 255.0;
            let color = vec3<f32>(f32(pixel.r), f32(pixel.g), f32(pixel.b)) / 255.0;
            let distance = length(color - key) / sqrt(3.0);
            let similarity = f32(parameters.values[filter_offset + 4]) / 1000.0;
            let smoothness = f32(parameters.values[filter_offset + 5]) / 1000.0;
            var alpha_factor = 1.0;
            if (distance <= similarity) {
                alpha_factor = 0.0;
            } else if (smoothness > 0.0 && distance < similarity + smoothness) {
                alpha_factor = (distance - similarity) / smoothness;
            }
            pixel.a = i32(floor(f32(pixel.a) * alpha_factor + 0.5));
        } else if (kind == 6) {
            let color = vec3<f32>(f32(pixel.r), f32(pixel.g), f32(pixel.b)) / 255.0;
            let luma = dot(color, vec3<f32>(0.2989, 0.5870, 0.1140));
            let luma_max = f32(parameters.values[filter_offset + 1]) / 1000.0;
            let luma_min = f32(parameters.values[filter_offset + 2]) / 1000.0;
            let luma_max_smooth = f32(parameters.values[filter_offset + 3]) / 1000.0;
            let luma_min_smooth = f32(parameters.values[filter_offset + 4]) / 1000.0;
            var lower = 0.0;
            if (luma_min_smooth <= 0.0) {
                if (luma >= luma_min) {
                    lower = 1.0;
                }
            } else {
                let position = clamp((luma - luma_min) / luma_min_smooth, 0.0, 1.0);
                lower = position * position * (3.0 - 2.0 * position);
            }
            var upper = 0.0;
            if (luma_max_smooth <= 0.0) {
                if (luma <= luma_max) {
                    upper = 1.0;
                }
            } else {
                let position = clamp(
                    (luma - (luma_max - luma_max_smooth)) / luma_max_smooth,
                    0.0,
                    1.0,
                );
                upper = 1.0 - position * position * (3.0 - 2.0 * position);
            }
            pixel.a = i32(floor(f32(pixel.a) * lower * upper + 0.5));
        } else if (kind == 7) {
            let color = vec3<f32>(f32(pixel.r), f32(pixel.g), f32(pixel.b)) / 255.0;
            let nonlinear = vec3<f32>(
                chroma_nonlinear_channel(color.r),
                chroma_nonlinear_channel(color.g),
                chroma_nonlinear_channel(color.b),
            );
            let key_color = vec3<f32>(
                f32(parameters.values[filter_offset + 1]),
                f32(parameters.values[filter_offset + 2]),
                f32(parameters.values[filter_offset + 3]),
            ) / 255.0;
            let key_nonlinear = vec3<f32>(
                chroma_nonlinear_channel(key_color.r),
                chroma_nonlinear_channel(key_color.g),
                chroma_nonlinear_channel(key_color.b),
            );
            let chroma = chroma_components(nonlinear);
            let key_chroma = chroma_components(key_nonlinear);
            let distance = length(chroma - key_chroma);
            let similarity = f32(parameters.values[filter_offset + 4]) / 1000.0;
            let smoothness = f32(parameters.values[filter_offset + 5]) / 1000.0;
            let spill = f32(parameters.values[filter_offset + 6]) / 1000.0;
            let base_mask = max(distance - similarity, 0.0);
            let full_mask = chroma_key_mask(base_mask, smoothness);
            let spill_mask = chroma_key_mask(base_mask, spill);
            let desaturated = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
            let spill_color = vec3<f32>(desaturated) +
                (color - vec3<f32>(desaturated)) * spill_mask;
            pixel.r = i32(floor(clamp(spill_color.r, 0.0, 1.0) * 255.0 + 0.5));
            pixel.g = i32(floor(clamp(spill_color.g, 0.0, 1.0) * 255.0 + 0.5));
            pixel.b = i32(floor(clamp(spill_color.b, 0.0, 1.0) * 255.0 + 0.5));
            pixel.a = i32(floor(f32(pixel.a) * full_mask + 0.5));
        } else if (kind == 8) {
            let center = vec4<f32>(pixel) / 255.0;
            let left_position = vec2<i32>(
                clamp(source_x - 1, 0, source_width - 1),
                source_y,
            );
            let right_position = vec2<i32>(
                clamp(source_x + 1, 0, source_width - 1),
                source_y,
            );
            let top_position = vec2<i32>(
                source_x,
                clamp(source_y - 1, 0, source_height - 1),
            );
            let bottom_position = vec2<i32>(
                source_x,
                clamp(source_y + 1, 0, source_height - 1),
            );
            let top_left_position = vec2<i32>(
                clamp(source_x - 1, 0, source_width - 1),
                clamp(source_y - 1, 0, source_height - 1),
            );
            let top_right_position = vec2<i32>(
                clamp(source_x + 1, 0, source_width - 1),
                clamp(source_y - 1, 0, source_height - 1),
            );
            let bottom_left_position = vec2<i32>(
                clamp(source_x - 1, 0, source_width - 1),
                clamp(source_y + 1, 0, source_height - 1),
            );
            let bottom_right_position = vec2<i32>(
                clamp(source_x + 1, 0, source_width - 1),
                clamp(source_y + 1, 0, source_height - 1),
            );
            let left = textureLoad(layer_texture, left_position, 0);
            let right = textureLoad(layer_texture, right_position, 0);
            let top = textureLoad(layer_texture, top_position, 0);
            let bottom = textureLoad(layer_texture, bottom_position, 0);
            let top_left = textureLoad(layer_texture, top_left_position, 0);
            let top_right = textureLoad(layer_texture, top_right_position, 0);
            let bottom_left = textureLoad(layer_texture, bottom_left_position, 0);
            let bottom_right = textureLoad(layer_texture, bottom_right_position, 0);
            let left_pixel = vec4<i32>(floor(left * 255.0 + vec4<f32>(0.5)));
            let right_pixel = vec4<i32>(floor(right * 255.0 + vec4<f32>(0.5)));
            let top_pixel = vec4<i32>(floor(top * 255.0 + vec4<f32>(0.5)));
            let bottom_pixel = vec4<i32>(floor(bottom * 255.0 + vec4<f32>(0.5)));
            let should_sharpen =
                (any(left_pixel != pixel) && any(right_pixel != pixel)) ||
                (any(top_pixel != pixel) && any(bottom_pixel != pixel));
            if (should_sharpen) {
                let top_left_pixel = vec4<i32>(floor(top_left * 255.0 + vec4<f32>(0.5)));
                let top_right_pixel = vec4<i32>(floor(top_right * 255.0 + vec4<f32>(0.5)));
                let bottom_left_pixel =
                    vec4<i32>(floor(bottom_left * 255.0 + vec4<f32>(0.5)));
                let bottom_right_pixel =
                    vec4<i32>(floor(bottom_right * 255.0 + vec4<f32>(0.5)));
                let kernel = vec4<f32>(
                    8 * pixel - left_pixel - right_pixel - top_pixel - bottom_pixel -
                        top_left_pixel - top_right_pixel - bottom_left_pixel - bottom_right_pixel,
                ) / 255.0;
                let strength = f32(parameters.values[filter_offset + 1]) / 1000.0;
                let sharpened = clamp(
                    center + kernel * strength,
                    vec4<f32>(0.0),
                    vec4<f32>(1.0),
                );
                pixel = vec4<i32>(floor(sharpened * 255.0 + vec4<f32>(0.5)));
            }
        } else if (kind == 9) {
            let multiply = vec3<f32>(
                f32(parameters.values[filter_offset + 1]),
                f32(parameters.values[filter_offset + 2]),
                f32(parameters.values[filter_offset + 3]),
            ) / 255.0;
            let add = vec3<f32>(
                f32(parameters.values[filter_offset + 4]),
                f32(parameters.values[filter_offset + 5]),
                f32(parameters.values[filter_offset + 6]),
            ) / 255.0;
            let color = clamp(
                vec3<f32>(f32(pixel.r), f32(pixel.g), f32(pixel.b)) / 255.0 * multiply + add,
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
            pixel.r = i32(floor(color.r * 255.0 + 0.5));
            pixel.g = i32(floor(color.g * 255.0 + 0.5));
            pixel.b = i32(floor(color.b * 255.0 + 0.5));
        } else if (kind == 10) {
            let offset_x = parameters.values[filter_offset + 1];
            let offset_y = parameters.values[filter_offset + 2];
            let sample_x = source_x + offset_x;
            let sample_y = source_y + offset_y;
            let looped = parameters.values[filter_offset + 3] != 0;
            if (looped) {
                var wrapped_x = sample_x % source_width;
                var wrapped_y = sample_y % source_height;
                if (wrapped_x < 0) { wrapped_x = wrapped_x + source_width; }
                if (wrapped_y < 0) { wrapped_y = wrapped_y + source_height; }
                let scrolled = textureLoad(layer_texture, vec2<i32>(wrapped_x, wrapped_y), 0);
                pixel = vec4<i32>(floor(scrolled * 255.0 + vec4<f32>(0.5)));
                pixel.a = pixel.a * parameters.values[10] / 255;
            } else if (sample_x < 0 || sample_x >= source_width ||
                       sample_y < 0 || sample_y >= source_height) {
                pixel = vec4<i32>(0);
            } else {
                let scrolled = textureLoad(layer_texture, vec2<i32>(sample_x, sample_y), 0);
                pixel = vec4<i32>(floor(scrolled * 255.0 + vec4<f32>(0.5)));
                pixel.a = pixel.a * parameters.values[10] / 255;
            }
        }
        filter_index = filter_index + 1;
    }
    if (pixel.a == 0) {
        pixel.r = 0;
        pixel.g = 0;
        pixel.b = 0;
    }
    return pixel;
}

@fragment
fn fs_replace(input: VertexOutput) -> @location(0) vec4<f32> {
    let position = vec2<i32>(input.position.xy);
    return vec4<f32>(layer_pixel(position)) / 255.0;
}

@fragment
fn fs_composite(input: VertexOutput) -> @location(0) vec4<f32> {
    let position = vec2<i32>(input.position.xy);
    let source = layer_pixel(position);
    if (source.a == 255) {
        return vec4<f32>(source) / 255.0;
    }
    let sampled_background = textureLoad(background_texture, position, 0);
    let background = vec4<i32>(floor(sampled_background * 255.0 + vec4<f32>(0.5)));
    if (source.a == 0) {
        return vec4<f32>(background) / 255.0;
    }
    let inverse_alpha = 255 - source.a;
    if (background.a == 255) {
        var output = vec4<i32>(0, 0, 0, 255);
        output.r = (source.r * source.a + background.r * inverse_alpha) / 255;
        output.g = (source.g * source.a + background.g * inverse_alpha) / 255;
        output.b = (source.b * source.a + background.b * inverse_alpha) / 255;
        return vec4<f32>(output) / 255.0;
    }
    let output_alpha = source.a + background.a * inverse_alpha / 255;
    if (output_alpha == 0) {
        return vec4<f32>(0.0);
    }
    let denominator = output_alpha * 255;
    let source_weight = source.a * 255;
    let background_weight = background.a * inverse_alpha;
    var output = vec4<i32>(0, 0, 0, output_alpha);
    output.r = (source.r * source_weight + background.r * background_weight) / denominator;
    output.g = (source.g * source_weight + background.g * background_weight) / denominator;
    output.b = (source.b * source_weight + background.b * background_weight) / denominator;
    return vec4<f32>(output) / 255.0;
}
