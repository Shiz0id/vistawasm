//! The Direct3D 11 command stream a real scene produces obeys Direct3D
//! 11's rules. A mock executor replays it and checks every record: objects
//! exist before use and match how they are used, views fit their
//! resources' bind flags, constant buffers are written whole, and no
//! subresource is bound as an input while it is an output, which Direct3D
//! 11 would quietly undo.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::ptr;

use vista_native::renderer::{
  vista_renderer_attach, vista_renderer_commands, vista_renderer_frame, VistaCommands,
  VistaOutputFormat,
};
use vista_native::{
  vista_engine_create, vista_engine_destroy, vista_engine_generate_fractal, vista_engine_set,
  VistaEngine, VistaStatus,
};

const BIND_VERTEX_BUFFER: u32 = 0x1;
const BIND_INDEX_BUFFER: u32 = 0x2;
const BIND_CONSTANT_BUFFER: u32 = 0x4;
const BIND_SHADER_RESOURCE: u32 = 0x8;
const BIND_RENDER_TARGET: u32 = 0x20;
const BIND_DEPTH_STENCIL: u32 = 0x40;
const BIND_UNORDERED_ACCESS: u32 = 0x80;
const MISC_DRAWINDIRECT_ARGS: u32 = 0x10;
const MISC_BUFFER_ALLOW_RAW_VIEWS: u32 = 0x20;
const OUTPUT: u32 = 1;

#[derive(Clone, Debug)]
enum Object {
  Buffer {
    size: u32,
    bind: u32,
    misc: u32,
  },
  Texture {
    layers: u32,
    mips: u32,
    bind: u32,
    three: bool,
  },
  /// A view: its resource and the subresources it covers.
  View {
    kind: u32,
    resource: u32,
    subresources: HashSet<u32>,
  },
  Sampler,
  Shader {
    stage: u32,
  },
  State,
}

#[derive(Default)]
struct Executor {
  objects: HashMap<u32, Object>,
  /// Bound views and buffers per (stage, slot kind, register).
  bound: HashMap<(u32, u32, u32), u32>,
  targets: Vec<u32>,
  depth: u32,
  graphics: Option<[u32; 7]>,
  compute: u32,
  vertex_buffers: HashMap<u32, u32>,
  index_buffer: u32,
  draws: usize,
  dispatches: usize,
  presents: usize,
  readbacks: usize,
  /// Reads to answer: buffer and size.
  waiting_reads: Vec<(u32, u32)>,
}

fn words(payload: &[u8]) -> Vec<u32> {
  payload
    .chunks_exact(4)
    .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    .collect()
}

impl Executor {
  fn object(&self, id: u32, what: &str) -> &Object {
    self
      .objects
      .get(&id)
      .unwrap_or_else(|| panic!("{what} uses #{id:x}, which does not exist"))
  }

  fn create(&mut self, id: u32, object: Object) {
    assert!(id > OUTPUT, "#{id:x} is reserved");
    assert!(
      self.objects.insert(id, object).is_none(),
      "#{id:x} was created twice"
    );
  }

  /// The subresources a view covers, as (resource, subresource).
  fn covers(&self, view: u32) -> Vec<(u32, u32)> {
    match self.objects.get(&view) {
      Some(Object::View {
        resource,
        subresources,
        ..
      }) => subresources.iter().map(|sub| (*resource, *sub)).collect(),
      // A constant buffer bound directly.
      Some(Object::Buffer { .. }) => vec![(view, 0)],
      _ => Vec::new(),
    }
  }

  fn outputs(&self, compute: bool) -> HashSet<(u32, u32)> {
    let mut out = HashSet::new();

    if compute {
      for ((stage, kind, _), view) in &self.bound {
        if *stage == 2 && *kind == 3 {
          out.extend(self.covers(*view));
        }
      }
    } else {
      for view in self.targets.iter().chain([&self.depth]) {
        out.extend(self.covers(*view));
      }
    }

    out
  }

  fn inputs(&self, stages: &[u32]) -> HashSet<(u32, u32)> {
    let mut out = HashSet::new();

    for ((stage, kind, _), view) in &self.bound {
      if stages.contains(stage) && *kind == 1 {
        out.extend(self.covers(*view));
      }
    }

    out
  }

  fn view(&mut self, id: u32, kind: u32, w: &[u32]) {
    let [resource, _format, dimension, a, b, c, d] = [w[1], w[2], w[3], w[4], w[5], w[6], w[7]];
    let (need, raw) = match kind {
      3 => (BIND_SHADER_RESOURCE, dimension == 11),
      4 => (BIND_UNORDERED_ACCESS, dimension == 1),
      5 => (BIND_RENDER_TARGET, false),
      _ => (BIND_DEPTH_STENCIL, false),
    };
    let subresources: HashSet<u32> = if resource == OUTPUT {
      assert_eq!(kind, 5, "the output may only be a render target");
      HashSet::from([0])
    } else {
      match self.object(resource, "a view").clone() {
        Object::Buffer { bind, misc, size } => {
          assert!(raw, "buffer #{resource:x} viewed as a texture");
          assert!(
            bind & need != 0,
            "buffer #{resource:x} lacks the bind flag for its view"
          );
          assert!(
            misc & MISC_BUFFER_ALLOW_RAW_VIEWS != 0,
            "raw view of #{resource:x} without the raw flag"
          );
          assert!(
            (a + b) * 4 <= size,
            "a raw view past the end of #{resource:x}"
          );
          HashSet::from([0])
        }
        Object::Texture {
          layers,
          mips,
          bind,
          three,
        } => {
          assert!(!raw, "texture #{resource:x} viewed as a buffer");
          assert!(
            bind & need != 0,
            "texture #{resource:x} lacks the bind flag for its view"
          );
          let (mip_range, layer_range) = match (kind, dimension) {
            // SRV 2D, 3D: mip, count.
            (3, 4) | (3, 8) => (a..a + b, 0..1),
            (3, 5) => (a..a + b, c..c + d),
            (4, 4) | (5, 4) | (6, 3) => (a..a + 1, 0..1),
            (4, 8) => (a..a + 1, 0..1),
            (4, 5) | (5, 5) | (6, 4) => (a..a + 1, b..b + c),
            other => panic!("unexpected view dimension {other:?}"),
          };
          assert!(
            mip_range.end <= mips,
            "a view past the last mip of #{resource:x}"
          );

          if !three {
            assert!(
              layer_range.end <= layers,
              "a view past the last layer of #{resource:x}"
            );
          }

          // A 2D view of a layered texture would need its array dimension.
          if matches!((kind, dimension), (3, 4) | (4, 4) | (5, 4) | (6, 3)) {
            assert_eq!(layers, 1, "a 2D view of layered texture #{resource:x}");
          }

          mip_range
            .flat_map(|mip| layer_range.clone().map(move |layer| mip + layer * mips))
            .collect()
        }
        other => panic!("#{resource:x} is {other:?}, not a resource"),
      }
    };
    self.create(
      id,
      Object::View {
        kind,
        resource,
        subresources,
      },
    );
  }

  fn run(&mut self, stream: &[u8]) {
    let mut at = 0;

    while at < stream.len() {
      let header = words(&stream[at..at + 8]);
      let (op, bytes) = (header[0], header[1] as usize);
      assert!(bytes.is_multiple_of(4), "a payload of {bytes} bytes");
      let payload = &stream[at + 8..at + 8 + bytes];
      at += 8 + bytes;
      let w = words(payload);
      self.record(op, &w, payload);
    }
  }

  fn record(&mut self, op: u32, w: &[u32], payload: &[u8]) {
    match op {
      1 => {
        assert!(w[1] > 0 && w[1].is_multiple_of(4), "buffer size {}", w[1]);

        if w[2] & BIND_CONSTANT_BUFFER != 0 {
          assert_eq!(
            w[2], BIND_CONSTANT_BUFFER,
            "constant buffers bind as nothing else"
          );
          assert_eq!(w[1] % 16, 0, "constant buffer size {}", w[1]);
        }

        assert!(w[2] != 0, "buffer #{:x} binds as nothing", w[0]);
        self.create(
          w[0],
          Object::Buffer {
            size: w[1],
            bind: w[2],
            misc: w[3],
          },
        );
      }
      2 => {
        assert!(w[7] != 0, "texture #{:x} binds as nothing", w[0]);
        self.create(
          w[0],
          Object::Texture {
            layers: w[4],
            mips: w[5],
            bind: w[7],
            three: w[1] == 3,
          },
        );
      }
      3..=6 => self.view(w[0], op, w),
      7 => self.create(w[0], Object::Sampler),
      8 => {
        assert_eq!(&payload[12..16], b"DXBC", "shader #{:x} is not DXBC", w[0]);
        self.create(w[0], Object::Shader { stage: w[1] });
      }
      9 => {
        assert!(matches!(
          self.object(w[1], "an input layout"),
          Object::Shader { stage: 0 }
        ));
        assert_eq!(w.len(), 3 + w[2] as usize * 6);
        self.create(w[0], Object::State);
      }
      10..=12 => self.create(w[0], Object::State),
      13 => {
        let gone = self.objects.remove(&w[0]);
        assert!(gone.is_some(), "#{:x} released but never created", w[0]);
      }
      20 => {
        if let Object::Buffer { size, bind, .. } = self.object(w[0], "a buffer write") {
          assert!(w[1] + w[2] <= *size, "a write past the end of #{:x}", w[0]);

          if bind & BIND_CONSTANT_BUFFER != 0 {
            assert!(
              w[1] == 0 && w[2] == *size,
              "a partial constant buffer write"
            );
          }
        } else {
          panic!("a buffer write to #{:x}, not a buffer", w[0]);
        }
      }
      21 => {
        assert!(matches!(
          self.object(w[0], "a texture write"),
          Object::Texture { .. }
        ));
      }
      22 => {
        self.object(w[0], "a copy");
        self.object(w[2], "a copy");
      }
      23 => {
        self.object(w[0], "a copy");
        self.object(w[5], "a copy");
      }
      24 => {
        self.object(w[0], "a read back");
        self.readbacks += 1;
        self.waiting_reads.push((w[0], w[1]));
      }
      30 => {
        self.targets = w[2..2 + w[0] as usize]
          .iter()
          .copied()
          .filter(|t| *t != 0)
          .collect();
        self.depth = w[1];
        let outputs = self.outputs(false);
        let clash = self.inputs(&[0, 1, 2]).intersection(&outputs).count();
        assert_eq!(
          clash, 0,
          "render targets set while bound as shader resources"
        );
      }
      31 | 32 => {
        self.object(w[0], "a clear");
      }
      33 => {}
      34 => {
        assert!(matches!(
          self.object(w[0], "a pipeline"),
          Object::Shader { stage: 0 }
        ));

        if w[1] != 0 {
          assert!(matches!(
            self.object(w[1], "a pipeline"),
            Object::Shader { stage: 1 }
          ));
        }

        self.graphics = Some([w[0], w[1], w[2], w[3], w[4], w[5], w[6]]);
      }
      35 => {
        assert!(matches!(
          self.object(w[0], "a dispatch"),
          Object::Shader { stage: 2 }
        ));
        self.compute = w[0];
      }
      36 => {
        let [stage, kind, slot, object] = [w[0], w[1], w[2], w[3]];

        if object == 0 {
          self.bound.remove(&(stage, kind, slot));
          return;
        }

        match (kind, self.object(object, "a binding")) {
          (0, Object::Buffer { bind, .. }) => assert!(bind & BIND_CONSTANT_BUFFER != 0),
          (1, Object::View { kind: 3, .. }) | (2, Object::Sampler) => {}
          (3, Object::View { kind: 4, .. }) => assert_eq!(stage, 2, "a UAV outside compute"),
          (kind, other) => panic!("{other:?} bound as slot kind {kind}"),
        }

        // Binding an input that is an output fails in Direct3D 11.
        if kind == 1 {
          let compute = stage == 2;
          let clash = self
            .covers(object)
            .into_iter()
            .filter(|sub| self.outputs(compute).contains(sub))
            .count();
          assert_eq!(clash, 0, "a shader resource bound while it is an output");
        }

        self.bound.insert((stage, kind, slot), object);
      }
      37 => {
        self.bound.clear();
        self.vertex_buffers.clear();
        self.index_buffer = 0;
      }
      38 => {
        match self.object(w[1], "a vertex buffer") {
          Object::Buffer { bind, .. } => assert!(bind & BIND_VERTEX_BUFFER != 0),
          other => panic!("{other:?} as a vertex buffer"),
        }

        self.vertex_buffers.insert(w[0], w[1]);
      }
      39 => {
        match self.object(w[0], "an index buffer") {
          Object::Buffer { bind, .. } => assert!(bind & BIND_INDEX_BUFFER != 0),
          other => panic!("{other:?} as an index buffer"),
        }

        self.index_buffer = w[0];
      }
      40..=43 => {
        assert!(self.graphics.is_some(), "a draw without a pipeline");
        assert!(
          !self.targets.is_empty() || self.depth != 0,
          "a draw without targets"
        );

        if op >= 42 {
          match self.object(w[0], "an indirect draw") {
            Object::Buffer { misc, .. } => assert!(misc & MISC_DRAWINDIRECT_ARGS != 0),
            other => panic!("{other:?} as indirect arguments"),
          }
        }

        if op == 41 || op == 43 {
          assert!(self.index_buffer != 0, "an indexed draw without indices");
        }

        let clash = self
          .inputs(&[0, 1])
          .intersection(&self.outputs(false))
          .count();
        assert_eq!(clash, 0, "a draw reads what it draws into");
        self.draws += 1;
      }
      44 => {
        assert!(self.compute != 0, "a dispatch without a shader");
        // Per resource, not per subresource: some runtimes unbind a
        // shader resource whose resource has any subresource bound as a
        // UAV.
        let read: HashSet<u32> = self
          .inputs(&[2])
          .into_iter()
          .map(|(resource, _)| resource)
          .collect();
        let written: HashSet<u32> = self
          .outputs(true)
          .into_iter()
          .map(|(resource, _)| resource)
          .collect();
        assert_eq!(
          read.intersection(&written).count(),
          0,
          "a dispatch reads a resource it writes"
        );
        self.dispatches += 1;
      }
      50 => self.presents += 1,
      51 => {}
      other => panic!("unknown record {other}"),
    }
  }
}

fn check(status: VistaStatus) {
  assert_eq!(status, VistaStatus::Ok, "{:?}", unsafe {
    std::ffi::CStr::from_ptr(vista_native::vista_last_error())
  });
}

fn stream(commands: &VistaCommands) -> &[u8] {
  // SAFETY: the library handed out `bytes` bytes at `data`.
  unsafe { std::slice::from_raw_parts(commands.data, commands.bytes) }
}

#[test]
fn a_scene_lowers_to_a_valid_direct3d_11_stream() {
  let mut engine: *mut VistaEngine = ptr::null_mut();
  // SAFETY: valid arguments throughout.
  unsafe {
    check(vista_engine_create(ptr::null(), &mut engine));
    check(vista_renderer_attach(
      engine,
      320,
      200,
      VistaOutputFormat::Bgra8,
    ));
    let terrain =
      CString::new(r#"{ "size": 128, "horizontalScaleMetres": 8, "seed": 7 }"#).unwrap();
    check(vista_engine_generate_fractal(
      engine,
      terrain.as_ptr(),
      None,
      ptr::null_mut(),
    ));
    let section = CString::new("camera").unwrap();
    let camera = CString::new(r#"{ "position": [-300, 250, 400], "target": [0, 40, 0] }"#).unwrap();
    check(vista_engine_set(engine, section.as_ptr(), camera.as_ptr()));
    let mut executor = Executor::default();
    let mut commands = VistaCommands {
      data: ptr::null(),
      bytes: 0,
    };
    check(vista_renderer_commands(engine, &mut commands));
    executor.run(stream(&commands));

    for frame in 0..6 {
      check(vista_renderer_frame(
        engine,
        f64::from(frame) * 16.0,
        &mut commands,
      ));
      executor.run(stream(&commands));

      // The draw counts, as a GPU that drew nothing would report them.
      for (buffer, size) in std::mem::take(&mut executor.waiting_reads) {
        let bytes = vec![0u8; size as usize];
        check(vista_native::renderer::vista_renderer_complete_read(
          engine,
          buffer,
          bytes.as_ptr().cast(),
          bytes.len(),
        ));
      }
    }

    assert_eq!(executor.presents, 6);
    assert!(executor.draws > 20, "only {} draws", executor.draws);
    assert!(
      executor.dispatches > 10,
      "only {} dispatches",
      executor.dispatches
    );
    // Answered at once, each frame reads its counts back.
    assert_eq!(
      executor.readbacks, 6,
      "the trees' draw counts are read back each frame"
    );

    let mut events = ptr::null_mut();
    check(vista_native::renderer::vista_engine_events_json(
      engine,
      &mut events,
    ));
    let text = std::ffi::CStr::from_ptr(events)
      .to_string_lossy()
      .into_owned();
    vista_native::vista_string_free(events);
    assert_eq!(text, r#"{"errors":[],"lost":null}"#);
    vista_engine_destroy(engine);
  }
}

/// Every embedded shader was compiled from the HLSL now in the
/// repository: run `ports/d3d11/tools/check-hlsl.sh` after translating.
#[test]
fn the_embedded_shaders_are_compiled_from_the_current_hlsl() {
  use vista_native::d3d11::{fnv1a, COMPILED, FILES};
  let hlsl = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ports/d3d11/hlsl");

  for file in FILES {
    let text = std::fs::read(hlsl.join(file.file)).unwrap();
    let stamp = COMPILED
      .iter()
      .find(|(name, _)| *name == file.file)
      .map(|(_, hash)| *hash);
    assert_eq!(
      stamp,
      Some(format!("{:016x}", fnv1a(&text)).as_str()),
      "{} was not compiled from its current HLSL: run ports/d3d11/tools/check-hlsl.sh",
      file.file
    );
  }
}
