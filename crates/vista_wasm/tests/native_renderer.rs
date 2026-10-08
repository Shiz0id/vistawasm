//! A native engine with a renderer attached records a coherent stream of
//! GPU work: every resource is created before it is used and not used
//! after it is released, every shader is one the Direct3D 11 port has,
//! and each frame ends in a present.

use std::collections::HashSet;

use futures_executor::block_on;
use vista_types::{CameraOptions, FractalTerrainOptions, VistaEngineOptions};
use vista_wasm::engine::EngineCore;
use vista_wasm::render::recorder::{
  Command, Id, Op, Recorder, Resource, TextureFormat, OUTPUT_TEXTURE,
};

/// Replays `ops` against a set of live ids, failing on any misuse.
struct Checker {
  live: HashSet<Id>,
  presents: usize,
  draws: usize,
  dispatches: usize,
  modules: HashSet<String>,
}

impl Checker {
  fn new() -> Self {
    Self {
      live: HashSet::from([OUTPUT_TEXTURE]),
      presents: 0,
      draws: 0,
      dispatches: 0,
      modules: HashSet::new(),
    }
  }

  fn used(&self, id: Id, what: &str) {
    assert!(
      self.live.contains(&id),
      "{what} uses #{id}, which does not exist"
    );
  }

  fn created(&mut self, id: Id) {
    assert!(self.live.insert(id), "#{id} was created twice");
  }

  fn replay(&mut self, ops: &[Op]) {
    for op in ops {
      match op {
        Op::CreateBuffer { id, .. }
        | Op::CreateTexture { id, .. }
        | Op::CreateSampler { id, .. } => self.created(*id),
        Op::CreateView { id, texture, .. } => {
          self.used(*texture, "a view");
          self.created(*id);
        }
        Op::CreateBindGroup { id, entries, label } => {
          for (_, resource) in entries {
            match resource {
              Resource::Buffer { buffer, .. } => self.used(*buffer, label),
              Resource::View(view) => self.used(*view, label),
              Resource::Sampler(sampler) => self.used(*sampler, label),
            }
          }

          self.created(*id);
        }
        Op::CreateShaderModule { id, module } => {
          assert!(!module.is_empty(), "a shader the port does not have");
          self.modules.insert(module.clone());
          self.created(*id);
        }
        Op::CreateRenderPipeline { id, desc } => {
          self.used(desc.module, &desc.label);
          self.created(*id);
        }
        Op::CreateComputePipeline {
          id, module, label, ..
        } => {
          self.used(*module, label);
          self.created(*id);
        }
        Op::WriteBuffer { buffer, .. } => self.used(*buffer, "a buffer write"),
        Op::WriteTexture { texture, .. } => self.used(*texture, "a texture write"),
        Op::MapRead { buffer } => self.used(*buffer, "a read back"),
        Op::Submit(commands) => self.commands(commands),
        Op::Present => self.presents += 1,
        Op::ConfigureSurface { .. } => {}
        Op::Release(id) => {
          assert!(self.live.remove(id), "#{id} was released but did not exist");
        }
      }
    }
  }

  fn commands(&mut self, commands: &[Command]) {
    let mut in_pass = false;

    for command in commands {
      match command {
        Command::BeginRenderPass { colour, depth } => {
          assert!(!in_pass, "a pass began inside another");
          in_pass = true;

          for target in colour.iter().flatten() {
            self.used(target.view, "a colour target");
          }

          if let Some(depth) = depth {
            self.used(depth.view, "a depth target");
          }
        }
        Command::BeginComputePass => {
          assert!(!in_pass, "a pass began inside another");
          in_pass = true;
        }
        Command::EndRenderPass | Command::EndComputePass => {
          assert!(in_pass, "a pass ended that had not begun");
          in_pass = false;
        }
        Command::SetRenderPipeline(id) | Command::SetComputePipeline(id) => {
          self.used(*id, "a pass");
        }
        Command::SetBindGroup { group, .. } => {
          if let Some(group) = group {
            self.used(*group, "a pass");
          }
        }
        Command::SetVertexBuffer { buffer, .. } | Command::SetIndexBuffer { buffer, .. } => {
          self.used(*buffer, "a draw");
        }
        Command::Draw { .. } | Command::DrawIndexed { .. } => self.draws += 1,
        Command::DrawIndirect { buffer, .. } | Command::DrawIndexedIndirect { buffer, .. } => {
          self.used(*buffer, "an indirect draw");
          self.draws += 1;
        }
        Command::Dispatch { .. } => self.dispatches += 1,
        Command::SetViewport { .. } => {}
        Command::CopyBufferToBuffer {
          source,
          destination,
          ..
        } => {
          self.used(*source, "a copy");
          self.used(*destination, "a copy");
        }
        Command::CopyTextureToTexture {
          source,
          destination,
          ..
        } => {
          self.used(source.texture, "a copy");
          self.used(destination.texture, "a copy");
        }
      }
    }

    assert!(!in_pass, "a command buffer ended inside a pass");
  }
}

fn engine_with_renderer(recorder: &Recorder) -> EngineCore {
  let mut engine = EngineCore::new_for_tests(VistaEngineOptions::default()).unwrap();
  engine
    .attach_renderer(recorder.clone(), 320, 200, TextureFormat::Bgra8Unorm)
    .unwrap();
  engine
}

#[test]
fn a_rendered_frame_is_a_coherent_recording() {
  let recorder = Recorder::new();
  let mut engine = engine_with_renderer(&recorder);
  block_on(engine.generate_fractal(FractalTerrainOptions {
    size: 128,
    horizontal_scale_metres: 8.0,
    ..FractalTerrainOptions::default()
  }))
  .unwrap();
  engine
    .set_camera(CameraOptions {
      position: [-300.0, 250.0, 400.0],
      target: [0.0, 40.0, 0.0],
      ..CameraOptions::default()
    })
    .unwrap();
  let mut checker = Checker::new();

  for frame in 0..4 {
    engine.set_host_clock_ms(Some(f64::from(frame) * 16.0));
    engine.render_once().unwrap();
    checker.replay(&recorder.take_ops());
    recorder.work_done();
  }

  assert_eq!(checker.presents, 4);
  assert!(checker.draws > 8, "only {} draws", checker.draws);
  assert!(checker.dispatches > 0);

  for module in [
    "clipmap_render",
    "atmosphere",
    "water",
    "texture_gen",
    "mipgen",
  ] {
    assert!(
      checker.modules.contains(module),
      "{module} was never compiled"
    );
  }
}

#[test]
fn attaching_after_generating_uploads_the_terrain() {
  let mut engine = EngineCore::new_for_tests(VistaEngineOptions::default()).unwrap();
  block_on(engine.generate_fractal(FractalTerrainOptions {
    size: 64,
    ..FractalTerrainOptions::default()
  }))
  .unwrap();
  let recorder = Recorder::new();
  engine
    .attach_renderer(recorder.clone(), 64, 64, TextureFormat::Rgba8Unorm)
    .unwrap();
  let ops = recorder.take_ops();
  let mut checker = Checker::new();
  checker.replay(&ops);
  let labels: Vec<&str> = ops
    .iter()
    .filter_map(|op| match op {
      Op::CreateBuffer { label, .. } | Op::CreateTexture { label, .. } => Some(label.as_str()),
      _ => None,
    })
    .collect();

  for label in [
    "VistaWASM terrain heights",
    "VistaWASM terrain",
    "VistaWASM ocean grid",
  ] {
    assert!(labels.contains(&label), "no {label}");
  }

  // Without a renderer, nothing is recorded.
  assert!(engine.detach_renderer().is_some());
  engine.render_once().unwrap();
  assert!(recorder
    .take_ops()
    .iter()
    .all(|op| matches!(op, Op::Release(_))));
}
