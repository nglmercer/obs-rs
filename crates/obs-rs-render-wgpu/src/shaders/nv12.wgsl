@group(0) @binding(0) var rgba: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> packed: array<u32>;

fn source_rgb(x: u32, y: u32) -> vec3<f32> {
    return textureLoad(rgba, vec2<i32>(i32(x), i32(y)), 0).rgb;
}

fn byte_for(index: u32, dimensions: vec2<u32>) -> u32 {
    let pixel_count = dimensions.x * dimensions.y;
    if index < pixel_count {
        let x = index % dimensions.x;
        let y = index / dimensions.x;
        let rgb = source_rgb(x, y);
        let luma = 16.0 + 65.738 * rgb.r + 129.057 * rgb.g + 25.064 * rgb.b;
        return u32(clamp(round(luma), 0.0, 255.0));
    }

    let chroma_index = index - pixel_count;
    let sample_index = chroma_index / 2u;
    let chroma_width = dimensions.x / 2u;
    let base_x = (sample_index % chroma_width) * 2u;
    let base_y = (sample_index / chroma_width) * 2u;
    let rgb = (source_rgb(base_x, base_y)
        + source_rgb(base_x + 1u, base_y)
        + source_rgb(base_x, base_y + 1u)
        + source_rgb(base_x + 1u, base_y + 1u)) * 0.25;
    if chroma_index % 2u == 0u {
        let u = 128.0 - 37.945 * rgb.r - 74.494 * rgb.g + 112.439 * rgb.b;
        return u32(clamp(round(u), 0.0, 255.0));
    }
    let v = 128.0 + 112.439 * rgb.r - 94.154 * rgb.g - 18.285 * rgb.b;
    return u32(clamp(round(v), 0.0, 255.0));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let word_index = id.x + id.y * 4194240u;
    let dimensions = textureDimensions(rgba);
    let byte_count = dimensions.x * dimensions.y * 3u / 2u;
    let first = word_index * 4u;
    if first >= byte_count {
        return;
    }
    var word = 0u;
    for (var lane = 0u; lane < 4u; lane = lane + 1u) {
        let index = first + lane;
        if index < byte_count {
            word = word | (byte_for(index, dimensions) << (lane * 8u));
        }
    }
    packed[word_index] = word;
}
