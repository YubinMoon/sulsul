//! 물방울 셰이더를 화면 밖 그림에 그리고 CPU로 읽어 옵니다.
//!
//! 앱 창과 같은 GPU 장치를 씁니다. wgpu는 잡지 않은 GPU 오류를 패닉으로 처리하므로, 여기서 하는 GPU 작업은
//! 모두 오류 범위([`ErrorScope`]) 안에서 합니다. 오류 범위는 스레드마다 따로라 화면 스레드의 그리기에는 영향이 없습니다.

use std::future::Future;
use std::num::NonZeroU64;
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use wgpu::naga;

use super::shader::{self, ShaderError, UNIFORM_SIZE};

/// 셰이더가 내는 값을 그대로 담습니다(브라우저 캔버스처럼 sRGB 변환 없음).
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// GPU가 한 장을 다 그릴 때까지 기다리는 최대 시간
const GPU_TIMEOUT: Duration = Duration::from_secs(2);

/// 그릴 한 장
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// 창 크기 (물리 픽셀)
    pub width: u32,
    pub height: u32,
    /// 물방울 크기를 정하는 무대 높이 (물리 픽셀). 창은 무대의 아래쪽 가운데만 덮습니다.
    pub stage_height: f32,
    /// 등장을 시작한 뒤 지난 시간 (초)
    pub time: f32,
    /// 터지기 시작한 뒤 지난 시간 (초). 음수면 아직 터지지 않았습니다.
    pub pop: f32,
    /// 후처리가 끝난 뒤 지난 시간 (초). 음수면 아직 후처리 중이고, 터진 물방울들이 고리를 이뤄 돕니다.
    pub done: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Shader(#[from] ShaderError),
    #[error("the GPU rejected the droplet shader: {0}")]
    Pipeline(String),
    #[error("GPU error while drawing the droplet: {0}")]
    Gpu(String),
    #[error("timed out waiting for the GPU")]
    Timeout,
    #[error("could not read the droplet image back: {0}")]
    Readback(String),
}

pub struct DropletRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    target: Option<Target>,
}

/// 그릴 그림과 읽어 올 버퍼. 창 크기가 바뀔 때만 다시 만듭니다.
struct Target {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    /// 읽어 올 버퍼의 한 줄 바이트 수 (wgpu가 요구하는 256바이트 단위로 늘림)
    padded_row: u32,
}

impl DropletRenderer {
    /// 셰이더를 컴파일하고 파이프라인을 만듭니다. 셰이더가 크기 때문에 화면 스레드가 아닌 곳에서 부릅니다.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self, RenderError> {
        let fragment_source = shader::fragment()?;
        let scope = ErrorScope::push(&device);
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sulsul overlay vertex"),
            source: wgpu::ShaderSource::Glsl {
                shader: shader::VERTEX.into(),
                stage: naga::ShaderStage::Vertex,
                defines: &[],
            },
        });
        let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sulsul overlay droplet"),
            source: wgpu::ShaderSource::Glsl {
                shader: fragment_source.into(),
                stage: naga::ShaderStage::Fragment,
                defines: &[],
            },
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sulsul overlay"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sulsul overlay"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sulsul overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sulsul overlay stage"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sulsul overlay"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        scope.finish().map_err(RenderError::Pipeline)?;
        Ok(Self {
            device,
            queue,
            pipeline,
            uniforms,
            bind_group,
            target: None,
        })
    }

    /// 한 장을 그리고, 위에서부터 한 줄씩 `row(줄 번호, RGBA)`로 넘깁니다.
    /// RGBA는 알파를 곱하지 않은 값이고 한 줄은 `frame.width * 4`바이트입니다.
    pub fn render(
        &mut self,
        frame: &Frame,
        row: impl FnMut(usize, &[u8]),
    ) -> Result<(), RenderError> {
        let result = self.draw(frame, row);
        if result.is_err() {
            // 읽어 오다 멈춘 버퍼를 다시 쓰지 않도록 다음 장에서 새로 만듭니다.
            self.target = None;
        }
        result
    }

    fn draw(
        &mut self,
        frame: &Frame,
        mut row: impl FnMut(usize, &[u8]),
    ) -> Result<(), RenderError> {
        let scope = ErrorScope::push(&self.device);
        let target = ensure_target(&self.device, &mut self.target, frame.width, frame.height);
        self.queue
            .write_buffer(&self.uniforms, 0, &uniform_bytes(frame));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sulsul overlay"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sulsul overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: Some(target.height),
                },
            },
            wgpu::Extent3d {
                width: target.width,
                height: target.height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.queue.submit([encoder.finish()]);

        let slice = target.readback.slice(..);
        let (mapped_tx, mapped) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = mapped_tx.send(result);
        });
        let polled = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(GPU_TIMEOUT),
        });
        scope.finish().map_err(RenderError::Gpu)?;
        match polled {
            Ok(_) => {}
            Err(wgpu::PollError::Timeout) => return Err(RenderError::Timeout),
            Err(error) => return Err(RenderError::Gpu(error.to_string())),
        }
        match mapped.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(RenderError::Readback(error.to_string())),
            Err(_) => return Err(RenderError::Timeout),
        }
        {
            let data = slice
                .get_mapped_range()
                .map_err(|error| RenderError::Readback(error.to_string()))?;
            let padded = target.padded_row as usize;
            let tight = target.width as usize * 4;
            for (y, line) in data.chunks(padded).take(target.height as usize).enumerate() {
                row(y, &line[..tight]);
            }
        }
        target.readback.unmap();
        Ok(())
    }
}

fn ensure_target<'a>(
    device: &wgpu::Device,
    slot: &'a mut Option<Target>,
    width: u32,
    height: u32,
) -> &'a Target {
    let width = width.max(1);
    let height = height.max(1);
    if slot
        .as_ref()
        .is_some_and(|target| target.width == width && target.height == height)
    {
        return slot.as_ref().expect("방금 확인한 그림이 없습니다");
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sulsul overlay"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let padded_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sulsul overlay readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    slot.insert(Target {
        width,
        height,
        texture,
        view,
        readback,
        padded_row,
    })
}

/// 셰이더의 uniform 블록 내용: `u_resolution`(창 너비, 무대 높이), `u_time`, `u_height`(창 높이), `u_pop`, `u_done`
fn uniform_bytes(frame: &Frame) -> [u8; UNIFORM_SIZE as usize] {
    let values = [
        frame.width as f32,
        frame.stage_height,
        frame.time,
        frame.height as f32,
        frame.pop,
        frame.done,
        0.0,
        0.0,
    ];
    let mut bytes = [0; UNIFORM_SIZE as usize];
    for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *chunk = value.to_ne_bytes();
    }
    bytes
}

/// 이 스레드에서 한 GPU 작업의 오류를 모읍니다.
struct ErrorScope {
    guards: Vec<wgpu::ErrorScopeGuard>,
}

impl ErrorScope {
    fn push(device: &wgpu::Device) -> Self {
        let guards = [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ]
        .into_iter()
        .map(|filter| device.push_error_scope(filter))
        .collect();
        Self { guards }
    }

    /// 범위를 닫고, 오류가 있었으면 첫 오류를 돌려줍니다.
    fn finish(self) -> Result<(), String> {
        let mut first = None;
        // 나중에 연 범위부터 닫아야 합니다.
        for guard in self.guards.into_iter().rev() {
            if let Some(error) = block_on(guard.pop()) {
                first.get_or_insert_with(|| error.to_string());
            }
        }
        first.map_or(Ok(()), Err)
    }
}

/// wgpu의 비동기 결과를 기다립니다. 데스크톱 wgpu는 결과를 곧바로 돌려주므로 실제로 기다리는 일은 거의 없습니다.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// 테스트용 GPU 장치. 쓸 수 있는 어댑터가 없으면 `None`입니다.
    pub fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    /// 한 장을 그려 위에서부터 이어 붙인 RGBA로 돌려줍니다.
    pub fn render_rgba(renderer: &mut DropletRenderer, frame: &Frame) -> Vec<u8> {
        let stride = frame.width as usize * 4;
        let mut rgba = vec![0; stride * frame.height as usize];
        renderer
            .render(frame, |y, line| {
                rgba[y * stride..(y + 1) * stride].copy_from_slice(line);
            })
            .expect("물방울을 그리지 못했습니다");
        rgba
    }

    #[test]
    fn renders_droplet_on_the_gpu() {
        let Some((device, queue)) = test_device() else {
            eprintln!("쓸 수 있는 GPU 어댑터가 없어 건너뜁니다");
            return;
        };
        let mut renderer =
            DropletRenderer::new(device, queue).expect("물방울 파이프라인을 만들지 못했습니다");
        // 무대 높이 800px일 때 지름이 약 74px이고, 1초 시각에는 기준 자리(화면 아래에서 0.42 단위 위)에 있습니다.
        let frame = Frame {
            width: 280,
            height: 190,
            stage_height: 800.0,
            time: 1.0,
            pop: -1.0,
            done: -1.0,
        };
        let rgba = render_rgba(&mut renderer, &frame);
        let alpha = |x: usize, y: usize| rgba[(y * frame.width as usize + x) * 4 + 3];
        let center_y = frame.height as usize - (0.42 / 4.0 * 800.0) as usize;
        // 본체는 가장 맑은 가운데도 42%는 가립니다.
        assert!(alpha(140, center_y) > 80, "물방울 가운데가 보이지 않습니다");
        assert_eq!(alpha(0, 0), 0, "창 모서리가 투명하지 않습니다");
        assert_eq!(alpha(279, 0), 0, "창 모서리가 투명하지 않습니다");

        // 시작 순간에는 물방울이 아직 수면 아래에 있습니다.
        let start = render_rgba(&mut renderer, &Frame { time: 0.0, ..frame });
        let start_alpha = start[(center_y * frame.width as usize + 140) * 4 + 3];
        assert_eq!(start_alpha, 0, "시작 순간에 물방울이 이미 떠 있습니다");

        // 터진 뒤 후처리를 기다리는 동안에는 작은 물방울들이 물방울이 있던 자리 둘레를 돌고, 가운데는 비어 있습니다.
        let waiting = render_rgba(&mut renderer, &Frame { pop: 2.0, ..frame });
        assert_eq!(
            waiting[(center_y * frame.width as usize + 140) * 4 + 3],
            0,
            "터진 뒤에도 물방울 본체가 남아 있습니다"
        );
        assert!(
            waiting.chunks(4).any(|pixel| pixel[3] > 80),
            "후처리를 기다리는 동안 도는 물방울이 보이지 않습니다"
        );

        // 후처리가 끝나면 물방울들이 가운데로 모여 하나로 합쳐집니다.
        let merged = render_rgba(
            &mut renderer,
            &Frame {
                pop: 2.33,
                done: 0.33,
                ..frame
            },
        );
        assert!(
            merged[(center_y * frame.width as usize + 140) * 4 + 3] > 50,
            "후처리가 끝난 뒤 물방울들이 가운데로 모이지 않습니다"
        );

        // 모인 물방울이 뿅 하고 사라지면 아무것도 남지 않습니다. 터지는 도중에 끝나도 마찬가지입니다.
        for (pop, done) in [(1.0, 0.7), (3.0, 1.0)] {
            let gone = render_rgba(&mut renderer, &Frame { pop, done, ..frame });
            assert!(
                gone.chunks(4).all(|pixel| pixel[3] == 0),
                "터진 지 {pop}초, 후처리가 끝난 지 {done}초 뒤에도 물방울이 남아 있습니다"
            );
        }
    }
}
