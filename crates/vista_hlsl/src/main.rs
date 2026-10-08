//! Translate VistaWASM's WGSL shaders to Shader Model 5.0 HLSL for a
//! Direct3D 11 renderer.
//!
//! Usage: `cargo run -p vista_hlsl [-- <output directory>]`. The default
//! output is `ports/d3d11/hlsl` in the repository.
//!
//! Each entry point gets its own file, because Direct3D 11 binds resources
//! per shader stage and has few slots (8 UAVs for compute on feature level
//! 11.0, 14 constant buffers, 16 samplers). Registers are numbered from 0
//! per class (b, t, s, u) over the resources that entry point uses, in
//! WGSL `(group, binding)` order, and `manifest.json` maps them back.
//! `override` constants are fixed per variant: one file per combination.
//!
//! Shader modules are composed as `render::shaders` composes them, from
//! the readable sources rather than the minified ones. Keep `MODULES` in
//! step with `render_source()` and `compute_source()` there.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use naga::back::hlsl::{self, BindTarget, ShaderModel};
use naga::valid::{Capabilities, GlobalUse, ValidationFlags, Validator};
use naga::{AddressSpace, Binding, BuiltIn, ImageClass, ShaderStage, StorageAccess, TypeInner};
use serde_json::{json, Value};

/// A shader module: its name and the files it is made of, in order.
struct Module {
  name: &'static str,
  parts: &'static [&'static str],
}

const MODULES: &[Module] = &[
  // Render shaders: the ground rules and the common prelude first.
  Module {
    name: "clipmap_render",
    parts: &["ground", "common", "materials", "clipmap_render"],
  },
  Module {
    name: "boulders",
    parts: &["ground", "common", "materials", "boulders"],
  },
  Module {
    name: "water",
    parts: &["ground", "common", "lattice", "water"],
  },
  Module {
    name: "trees",
    parts: &["ground", "common", "trees"],
  },
  Module {
    name: "grass_instances",
    parts: &["ground", "common", "grass_instances"],
  },
  Module {
    name: "atmosphere",
    parts: &["ground", "common", "atmosphere"],
  },
  // Compute shaders.
  Module {
    name: "tree_cull",
    parts: &["lattice", "tree_cull"],
  },
  Module {
    name: "tree_generate",
    parts: &["ground", "lattice", "generate_common", "tree_generate"],
  },
  Module {
    name: "grass_generate",
    parts: &["ground", "lattice", "generate_common", "grass_generate"],
  },
  Module {
    name: "boulder_generate",
    parts: &["ground", "lattice", "generate_common", "boulder_generate"],
  },
  Module {
    name: "grounding",
    parts: &["ground", "grounding"],
  },
  Module {
    name: "terrain_shadow",
    parts: &["terrain_shadow"],
  },
  Module {
    name: "surface_weather",
    parts: &["surface_weather"],
  },
  Module {
    name: "texture_gen",
    parts: &["texture_gen"],
  },
  Module {
    name: "mipgen",
    parts: &["mipgen"],
  },
  Module {
    name: "hydraulic_erosion",
    parts: &["hydraulic_erosion"],
  },
  Module {
    name: "thermal_erosion",
    parts: &["thermal_erosion"],
  },
  Module {
    name: "normals",
    parts: &["normals"],
  },
];

/// Modules whose loops are all kept rolled (see `write_entry`).
const ROLLED_MODULES: &[&str] = &["texture_gen"];

/// Direct3D 11 slots per stage at feature level 11.0.
const MAX_CONSTANT_BUFFERS: usize = 14;
const MAX_SAMPLERS: usize = 16;
const MAX_SHADER_RESOURCES: usize = 128;
const MAX_UAVS: usize = 8;

type Failure = Box<dyn std::error::Error>;

fn main() {
  if let Err(error) = run() {
    eprintln!("vista_hlsl: {error}");
    std::process::exit(1);
  }
}

fn run() -> Result<(), Failure> {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
  let out = std::env::args()
    .nth(1)
    .map_or_else(|| root.join("ports/d3d11/hlsl"), PathBuf::from);
  let shaders = root.join("crates/vista_wasm/src/shaders");

  // Old output goes first, so a renamed entry point leaves no stale file.
  if out.exists() {
    fs::remove_dir_all(&out)?;
  }

  fs::create_dir_all(&out)?;
  let mut manifest = Vec::new();

  for module in MODULES {
    let mut source = String::new();

    for part in module.parts {
      let path = shaders.join(format!("{part}.wgsl"));
      source.push_str(
        &fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?,
      );
      source.push('\n');
    }

    let entries = translate(module, &source, &out)?;
    println!("{}: {} files", module.name, entries.len());
    manifest.extend(entries);
  }

  let manifest = json!({
    "generator": "vista_hlsl",
    "naga": "30",
    "shaderModel": "5.0",
    "files": manifest,
  });
  fs::write(
    out.join("manifest.json"),
    serde_json::to_string_pretty(&manifest)? + "\n",
  )?;
  fs::write(out.join("compile-fxc.ps1"), fxc_script(&manifest))?;
  Ok(())
}

/// Translate every entry point of `module`, in every combination of its
/// `override` constants, and return their manifest entries.
fn translate(module: &Module, source: &str, out: &Path) -> Result<Vec<Value>, Failure> {
  let parsed = naga::front::wgsl::parse_str(source).map_err(|error| {
    format!(
      "{} failed to parse: {}",
      module.name,
      error.emit_to_string(source)
    )
  })?;
  let info = Validator::new(ValidationFlags::all(), Capabilities::default())
    .validate(&parsed)
    .map_err(|error| format!("{} failed validation: {error:?}", module.name))?;
  let overrides: Vec<(String, bool)> = parsed
    .overrides
    .iter()
    .filter_map(|(_, item)| {
      let name = item.name.clone()?;
      let default = item
        .init
        .and_then(|init| match parsed.global_expressions[init] {
          naga::Expression::Literal(naga::Literal::Bool(value)) => Some(value),
          _ => None,
        })
        .unwrap_or(false);
      Some((name, default))
    })
    .collect();
  let directory = out.join(module.name);
  fs::create_dir_all(&directory)?;
  let mut entries = Vec::new();

  for combination in 0..(1u32 << overrides.len()) {
    let values: Vec<(String, bool, bool)> = overrides
      .iter()
      .enumerate()
      .map(|(bit, (name, default))| (name.clone(), combination & (1 << bit) != 0, *default))
      .collect();
    let constants: naga::back::PipelineConstants = values
      .iter()
      .map(|(name, value, _)| (name.clone(), if *value { 1.0 } else { 0.0 }))
      .collect();
    // Only the values that differ from the defaults name the variant.
    let suffix: String = values
      .iter()
      .filter(|(_, value, default)| value != default)
      .map(|(name, value, _)| format!(".{name}-{}", u8::from(*value)))
      .collect();

    for entry in &parsed.entry_points {
      let (fixed, fixed_info) = naga::back::pipeline_constants::process_overrides(
        &parsed,
        &info,
        Some((entry.stage, entry.name.as_str())),
        &constants,
      )
      .map_err(|error| format!("{}::{}: {error:?}", module.name, entry.name))?;
      let index = fixed
        .entry_points
        .iter()
        .position(|candidate| candidate.name == entry.name && candidate.stage == entry.stage)
        .ok_or_else(|| format!("{}::{} vanished", module.name, entry.name))?;
      let file = format!("{}{suffix}.hlsl", entry.name);
      let mut record = write_entry(
        &fixed,
        &fixed_info,
        index,
        &directory.join(&file),
        module,
        &values,
      )?;
      record["file"] = json!(format!("{}/{file}", module.name));
      entries.push(record);
    }
  }

  Ok(entries)
}

/// One resource an entry point binds.
struct Slot {
  group: u32,
  binding: u32,
  name: String,
  class: char,
  kind: String,
  used: bool,
  /// A storage texture the entry point reads, and its format: Direct3D
  /// 11.0 only reads 32-bit single-channel UAVs.
  typed_load: Option<String>,
}

fn write_entry(
  module: &naga::Module,
  info: &naga::valid::ModuleInfo,
  index: usize,
  path: &Path,
  source: &Module,
  overrides: &[(String, bool, bool)],
) -> Result<Value, Failure> {
  let entry = &module.entry_points[index];
  let uses = info.get_entry_point(index);
  let mut slots: Vec<Slot> = module
    .global_variables
    .iter()
    .filter_map(|(handle, global)| {
      let binding = global.binding.as_ref()?;
      let used = !uses[handle].is_empty();
      let inner = &module.types[global.ty].inner;
      let (class, kind, typed_load) = match (global.space, inner) {
        (AddressSpace::Uniform, _) => ('b', "constant buffer".to_string(), None),
        (AddressSpace::Storage { access }, _) if access.contains(StorageAccess::STORE) => {
          ('u', "RWByteAddressBuffer".to_string(), None)
        }
        (AddressSpace::Storage { .. }, _) => ('t', "ByteAddressBuffer".to_string(), None),
        (_, TypeInner::Sampler { comparison: true }) => {
          ('s', "SamplerComparisonState".to_string(), None)
        }
        (_, TypeInner::Sampler { comparison: false }) => ('s', "SamplerState".to_string(), None),
        (
          _,
          TypeInner::Image {
            dim,
            arrayed,
            class: ImageClass::Storage { format, access },
          },
        ) => {
          let reads =
            access.contains(StorageAccess::LOAD) && uses[handle].contains(GlobalUse::READ);
          let narrow = !matches!(
            format,
            naga::StorageFormat::R32Uint
              | naga::StorageFormat::R32Sint
              | naga::StorageFormat::R32Float
          );
          let kind = format!(
            "RWTexture{}{} ({format:?})",
            dimension(*dim),
            if *arrayed { "Array" } else { "" }
          );
          (
            'u',
            kind,
            (reads && narrow && used).then(|| format!("{format:?}")),
          )
        }
        (
          _,
          TypeInner::Image {
            dim,
            arrayed,
            class,
            ..
          },
        ) => {
          let kind = format!(
            "Texture{}{} ({class:?})",
            dimension(*dim),
            if *arrayed { "Array" } else { "" }
          );
          ('t', kind, None)
        }
        (space, other) => ('t', format!("{space:?} {other:?}"), None),
      };
      Some(Slot {
        group: binding.group,
        binding: binding.binding,
        name: global.name.clone().unwrap_or_default(),
        class,
        kind,
        used,
        typed_load,
      })
    })
    .collect();
  // Used resources take the lowest registers, in WGSL order.
  slots.sort_by_key(|slot| (!slot.used, slot.group, slot.binding));
  let mut next: BTreeMap<char, u32> = BTreeMap::new();
  let mut binding_map = hlsl::BindingMap::default();
  let mut registers = Vec::new();

  for slot in &slots {
    let register = next.entry(slot.class).or_insert(0);
    binding_map.insert(
      naga::ResourceBinding {
        group: slot.group,
        binding: slot.binding,
      },
      BindTarget {
        space: 0,
        register: *register,
        binding_array_size: None,
        dynamic_storage_buffer_offsets_index: None,
        restrict_indexing: false,
      },
    );
    registers.push(*register);
    *register += 1;
  }

  // Placeholders: `direct_samplers` removes the index buffers naga
  // declares with these.
  let sampler_buffer_binding_map = slots
    .iter()
    .filter(|slot| slot.class == 's')
    .map(|slot| {
      (
        hlsl::SamplerIndexBufferKey { group: slot.group },
        BindTarget {
          space: 0,
          register: 120 + slot.group,
          binding_array_size: None,
          dynamic_storage_buffer_offsets_index: None,
          restrict_indexing: false,
        },
      )
    })
    .collect();
  let options = hlsl::Options {
    shader_model: ShaderModel::V5_0,
    binding_map,
    sampler_buffer_binding_map,
    fake_missing_bindings: false,
    zero_initialize_workgroup_memory: true,
    // Naga bounds every loop with a 64-bit counter, against Direct3D 12
    // drivers that assume loops end. fxc cannot unroll through it, and
    // refuses some loops it would otherwise compile.
    force_loop_bounding: false,
    ..Default::default()
  };
  let pipeline = hlsl::PipelineOptions {
    entry_point: Some((entry.stage, entry.name.clone())),
  };
  let mut text = String::new();
  let reflection = hlsl::Writer::new(&mut text, &options, &pipeline)
    .write(module, info, None)
    .map_err(|error| format!("{}::{}: {error}", source.name, entry.name))?;
  let hlsl_name = reflection
    .entry_point_names
    .into_iter()
    .next()
    .ok_or("no entry point written")?
    .map_err(|error| format!("{}::{}: {error:?}", source.name, entry.name))?;
  let mut text = two_space_indent(&direct_samplers(&text)?);

  // fxc refuses a loop in divergent flow whose exit depends on data read
  // from a UAV (X3671) unless the loop says that is intended.
  if entry.stage == ShaderStage::Compute {
    text = text.replace("while(true) {", "[allow_uav_condition] while(true) {");
  }

  // fxc tries to unroll every loop whose count it can work out. In the
  // texture bake's large shaders that takes it far longer than the bake
  // gains, which runs once: gen_flora alone compiles in 3 minutes rolled,
  // and had not finished after 15 unrolled.
  if ROLLED_MODULES.contains(&source.name) {
    text = text.replace("while(true) {", "[loop] while(true) {");
  }

  let mut hazards = Vec::new();
  let used: Vec<(&Slot, u32)> = slots
    .iter()
    .zip(registers.iter().copied())
    .filter(|(slot, _)| slot.used)
    .collect();

  for (class, limit, what) in [
    ('b', MAX_CONSTANT_BUFFERS, "constant buffers"),
    ('s', MAX_SAMPLERS, "samplers"),
    ('t', MAX_SHADER_RESOURCES, "shader resources"),
    ('u', MAX_UAVS, "UAVs"),
  ] {
    let count = used.iter().filter(|(slot, _)| slot.class == class).count();

    if count > limit {
      hazards.push(format!(
        "uses {count} {what}; Direct3D 11.0 allows {limit} per stage (11.1 allows 64 UAVs)."
      ));
    }
  }

  if entry.stage == ShaderStage::Fragment && used.iter().any(|(slot, _)| slot.class == 'u') {
    hazards.push("writes UAVs from a pixel shader: they share the 8 output slots with render targets on 11.0, so bind them after the targets with OMSetRenderTargetsAndUnorderedAccessViews.".to_string());
  }

  for (slot, register) in &used {
    if let Some(format) = &slot.typed_load {
      hazards.push(format!(
        "reads storage texture `{}` (u{register}, {format}): Direct3D 11.0 only reads R32 UAVs. Check D3D11_FEATURE_DATA_D3D11_OPTIONS2::TypedUAVLoadAdditionalFormats, or read a copy through an SRV, or pack it into an R32_UINT texture.",
        slot.name
      ));
    }
  }

  let [x, y, z] = entry.workgroup_size;

  if entry.stage == ShaderStage::Compute && (x * y * z > 1024 || z > 64) {
    hazards.push(format!(
      "workgroup size {x}x{y}x{z} exceeds Direct3D 11's limits."
    ));
  }

  let builtins = entry_builtins(module, entry);

  if builtins.contains(&BuiltIn::InstanceIndex) {
    hazards.push("reads instance_index: SV_InstanceID does not include StartInstanceLocation in Direct3D 11, unlike WebGPU's first_instance. Draw with StartInstanceLocation 0, or add the offset from a constant buffer.".to_string());
  }

  if builtins.contains(&BuiltIn::VertexIndex) {
    hazards.push("reads vertex_index: SV_VertexID does not include BaseVertexLocation in Direct3D 11. Draw with base vertex 0, or add it from a constant buffer.".to_string());
  }

  let profile = match entry.stage {
    ShaderStage::Vertex => "vs_5_0",
    ShaderStage::Fragment => "ps_5_0",
    ShaderStage::Compute => "cs_5_0",
    _ => "unsupported",
  };
  let mut header = String::new();
  writeln!(
    header,
    "// Generated by vista_hlsl from VistaWASM's WGSL. Do not edit by hand:"
  )?;
  writeln!(
    header,
    "// change the WGSL and run `cargo run -p vista_hlsl`."
  )?;
  writeln!(header, "// Licence: AGPL-3.0-only, as VistaWASM.")?;
  writeln!(header, "//")?;
  writeln!(
    header,
    "// Module: {} ({})",
    source.name,
    source.parts.join(" + ")
  )?;
  writeln!(
    header,
    "// Entry point: {hlsl_name}, compile with /T {profile} /E {hlsl_name}"
  )?;

  for (name, value, _) in overrides {
    writeln!(header, "// override {name} = {value}")?;
  }

  writeln!(header, "//")?;
  writeln!(header, "// Resources (WGSL group/binding -> register):")?;

  for (slot, register) in &used {
    writeln!(
      header,
      "//   @group({}) @binding({}) {} -> {}{register}  {}",
      slot.group, slot.binding, slot.name, slot.class, slot.kind
    )?;
  }

  for hazard in &hazards {
    writeln!(header, "// D3D11: {hazard}")?;
  }

  writeln!(header)?;
  fs::write(path, header + &text)?;

  Ok(json!({
    "module": source.name,
    "entryPoint": hlsl_name,
    "wgslEntryPoint": entry.name,
    "stage": format!("{:?}", entry.stage).to_lowercase(),
    "profile": profile,
    "workgroupSize": (entry.stage == ShaderStage::Compute).then_some(entry.workgroup_size),
    "overrides": overrides.iter().map(|(name, value, _)| (name.clone(), json!(value))).collect::<serde_json::Map<_, _>>(),
    "bindings": used.iter().map(|(slot, register)| json!({
      "group": slot.group,
      "binding": slot.binding,
      "name": slot.name,
      "register": format!("{}{register}", slot.class),
      "kind": slot.kind,
    })).collect::<Vec<_>>(),
    "hazards": hazards,
  }))
}

/// The built-in inputs an entry point reads, through its arguments or
/// the members of a struct argument.
fn entry_builtins(module: &naga::Module, entry: &naga::EntryPoint) -> Vec<BuiltIn> {
  let mut found = Vec::new();

  for argument in &entry.function.arguments {
    if let Some(Binding::BuiltIn(builtin)) = argument.binding {
      found.push(builtin);
    }

    if let TypeInner::Struct { members, .. } = &module.types[argument.ty].inner {
      for member in members {
        if let Some(Binding::BuiltIn(builtin)) = member.binding {
          found.push(builtin);
        }
      }
    }
  }

  found
}

/// Naga binds samplers through a Direct3D 12 sampler heap and an index
/// buffer. Direct3D 11 has neither, so each sampler becomes a plain
/// `register(sN)` declaration and the heap goes.
fn direct_samplers(text: &str) -> Result<String, Failure> {
  let mut out = String::with_capacity(text.len());

  for line in text.lines() {
    let trimmed = line.trim_start();

    // The heaps and their index buffers.
    if (trimmed.starts_with("SamplerState nagaSamplerHeap")
      || trimmed.starts_with("SamplerComparisonState nagaComparisonSamplerHeap")
      || (trimmed.starts_with("StructuredBuffer<uint> nagaGroup")
        && trimmed.contains("SamplerIndexArray")))
      && trimmed.ends_with(';')
    {
      continue;
    }

    // `static const SamplerState name = nagaSamplerHeap[nagaGroup0SamplerIndexArray[3]];`
    if let Some(rest) = trimmed.strip_prefix("static const ") {
      if rest.contains("SamplerHeap[") {
        let mut words = rest.split_whitespace();
        let ty = words.next().ok_or("malformed sampler line")?;
        let name = words.next().ok_or("malformed sampler line")?;
        let register = sampler_register(rest)?;
        writeln!(out, "{ty} {name} : register(s{register});")?;
        continue;
      }
    }

    out.push_str(line);
    out.push('\n');
  }

  if out.contains("SamplerHeap") || out.contains("SamplerIndexArray") {
    return Err("a sampler heap reference survived the Direct3D 11 rewrite".into());
  }

  Ok(out)
}

/// HLSL's name for a texture dimension.
fn dimension(dim: naga::ImageDimension) -> &'static str {
  match dim {
    naga::ImageDimension::D1 => "1D",
    naga::ImageDimension::D2 => "2D",
    naga::ImageDimension::D3 => "3D",
    naga::ImageDimension::Cube => "Cube",
  }
}

/// Naga indents with 4 spaces; the project uses 2.
fn two_space_indent(text: &str) -> String {
  let mut out = String::with_capacity(text.len());

  for line in text.lines() {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    out.push_str(&" ".repeat(spaces / 2 + spaces % 2));
    out.push_str(&line[spaces..]);
    out.push('\n');
  }

  out
}

/// The register naga indexed the sampler index buffer with, which is the
/// sampler's register from the binding map.
fn sampler_register(line: &str) -> Result<u32, Failure> {
  let inner = line
    .rsplit_once("IndexArray[")
    .map(|(_, rest)| rest)
    .and_then(|rest| rest.split(']').next())
    .ok_or("malformed sampler line")?;
  Ok(inner.trim().parse()?)
}

/// A PowerShell script that compiles every file with `fxc`, so a Windows
/// machine can check the translation in one step.
fn fxc_script(manifest: &Value) -> String {
  let mut script = [
    "# Generated by vista_hlsl. Compiles every translated shader with fxc",
    "# (from the Windows SDK) and stops at the first failure.",
    "# Run from this directory: powershell -File compile-fxc.ps1",
    "$ErrorActionPreference = \"Stop\"",
    "New-Item -ItemType Directory -Force -Path cso | Out-Null",
    "",
  ]
  .join("\n");

  for file in manifest["files"].as_array().into_iter().flatten() {
    let (Some(path), Some(entry), Some(profile)) = (
      file["file"].as_str(),
      file["entryPoint"].as_str(),
      file["profile"].as_str(),
    ) else {
      continue;
    };
    let output = path.replace('/', "__").replace(".hlsl", ".cso");
    let _ = writeln!(
      script,
      "fxc /nologo /O3 /T {profile} /E {entry} /Fo \"cso/{output}\" \"{path}\"\nif ($LASTEXITCODE -ne 0) {{ throw \"{path} failed\" }}"
    );
  }

  script
}
