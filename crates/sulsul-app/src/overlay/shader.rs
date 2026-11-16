//! 미리보기의 물방울 셰이더(WebGL 2용 GLSL ES 3.00)를 wgpu가 읽는 GLSL 4.50으로 바꿉니다.
//!
//! 미리보기와 같은 파일(`prototypes/liquid-droplet/water.frag`)을 빌드할 때 넣고, 본문은 그대로 둔 채 선언만 바꿉니다.
//! - 따로 선언된 uniform을 uniform 블록 하나로 묶고, 창 높이(`u_height`)를 더합니다.
//!   `u_resolution`은 창 크기가 아니라 물방울 크기를 정하는 무대 크기이고, 창은 무대의 아래쪽 가운데만 덮습니다.
//! - WebGL의 `gl_FragCoord`는 원점이 왼쪽 아래이고 wgpu는 왼쪽 위라서 세로를 뒤집습니다.
//! - 렌즈(`LENS`)는 정의하지 않습니다. 렌즈는 물방울 뒤의 화면 그림이 있어야 그릴 수 있습니다.

/// 미리보기와 함께 쓰는 셰이더 원본
const SOURCE: &str = include_str!("../../../../prototypes/liquid-droplet/water.frag");

/// uniform 블록의 바이트 크기 (std140: `vec2 u_resolution`, `float u_time`, `float u_height`, `float u_pop`,
/// `float u_done`을 16바이트 단위로 늘림)
pub const UNIFORM_SIZE: u64 = 32;

/// 화면을 덮는 삼각형 하나를 그리는 꼭짓점 셰이더 (미리보기의 꼭짓점 셰이더와 같음)
pub const VERTEX: &str = "#version 450
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

const UNIFORMS: &str = "layout(set = 0, binding = 0) uniform Stage {
    vec2 u_resolution;
    float u_time;
    float u_height;
    float u_pop;
    float u_done;
};";

/// 줄 단위로 바꿀 선언 (원본 줄, 바꿀 내용)
const DECLARATIONS: [(&str, &str); 7] = [
    ("#version 300 es", "#version 450"),
    ("precision highp float;", ""),
    ("uniform vec2 u_resolution;", UNIFORMS),
    ("uniform float u_time;", ""),
    ("uniform float u_pop;", ""),
    ("uniform float u_done;", ""),
    (
        "out vec4 fragColor;",
        "layout(location = 0) out vec4 fragColor;",
    ),
];

const FRAG_COORD: &str = "gl_FragCoord.xy";
const FRAG_COORD_FLIPPED: &str = "vec2(gl_FragCoord.x, u_height - gl_FragCoord.y)";

#[derive(Debug, thiserror::Error)]
#[error("the droplet shader no longer contains `{0}`")]
pub struct ShaderError(&'static str);

/// 앱에서 쓸 조각 셰이더
pub fn fragment() -> Result<String, ShaderError> {
    translate(SOURCE)
}

fn translate(source: &str) -> Result<String, ShaderError> {
    let mut found = [false; DECLARATIONS.len()];
    let mut out = String::with_capacity(source.len() + UNIFORMS.len());
    for line in source.lines() {
        let trimmed = line.trim();
        match DECLARATIONS.iter().position(|(from, _)| *from == trimmed) {
            Some(index) if !found[index] => {
                found[index] = true;
                out.push_str(DECLARATIONS[index].1);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    if let Some(index) = found.iter().position(|found| !found) {
        return Err(ShaderError(DECLARATIONS[index].0));
    }
    if !out.contains(FRAG_COORD) {
        return Err(ShaderError(FRAG_COORD));
    }
    Ok(out.replace(FRAG_COORD, FRAG_COORD_FLIPPED))
}

#[cfg(test)]
mod tests {
    use wgpu::naga;

    use super::*;

    /// wgpu와 같은 방법(naga)으로 읽고 검사합니다. GPU 없이 돕니다.
    fn validate(source: &str, stage: naga::ShaderStage) {
        let module = naga::front::glsl::Frontend::default()
            .parse(&naga::front::glsl::Options::from(stage), source)
            .unwrap_or_else(|error| {
                panic!(
                    "셰이더를 읽지 못했습니다:\n{}",
                    error.emit_to_string(source)
                )
            });
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|error| {
            panic!(
                "셰이더 검사에 실패했습니다:\n{}",
                error.emit_to_string(source)
            )
        });
    }

    #[test]
    fn droplet_shader_translates_and_validates() {
        let source = fragment().expect("물방울 셰이더의 선언을 바꾸지 못했습니다");
        assert!(source.starts_with("#version 450\n"));
        assert!(!source.contains("gl_FragCoord.xy"));
        validate(&source, naga::ShaderStage::Fragment);
    }

    #[test]
    fn vertex_shader_validates() {
        validate(VERTEX, naga::ShaderStage::Vertex);
    }

    /// 셰이더의 `const float 이름 = 값;` 줄에서 값을 읽습니다.
    fn constant(name: &str) -> f32 {
        let prefix = format!("const float {name} = ");
        SOURCE
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(';'))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("셰이더에서 {name} 값을 찾지 못했습니다"))
    }

    #[test]
    fn overlay_timing_matches_the_shader() {
        assert_eq!(constant("POP_LENGTH"), crate::overlay::POP_LENGTH);
        assert_eq!(constant("DONE_LENGTH"), crate::overlay::DONE_LENGTH);
    }

    #[test]
    fn missing_declaration_is_reported() {
        let error = translate("void main() {}\n").expect_err("선언이 없는데 통과했습니다");
        assert_eq!(error.0, "#version 300 es");
    }
}
