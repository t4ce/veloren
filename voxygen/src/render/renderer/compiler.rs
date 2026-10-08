use common_base::prof_span;
#[cfg(not(feature = "precompiled-shaders"))]
use tracing::info;

use crate::render::RenderError;

pub(super) enum ShaderStage {
    Vertex,
    Fragment,
}

impl From<ShaderStage> for wgpu::naga::ShaderStage {
    fn from(value: ShaderStage) -> Self {
        match value {
            ShaderStage::Vertex => wgpu::naga::ShaderStage::Vertex,
            ShaderStage::Fragment => wgpu::naga::ShaderStage::Fragment,
        }
    }
}

impl From<ShaderStage> for shaderc::ShaderKind {
    fn from(value: ShaderStage) -> Self {
        match value {
            ShaderStage::Vertex => shaderc::ShaderKind::Vertex,
            ShaderStage::Fragment => shaderc::ShaderKind::Fragment,
        }
    }
}

pub(super) trait Compiler {
    fn create_shader_module(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        stage: ShaderStage,
        name: &str,
    ) -> Result<wgpu::ShaderModule, RenderError>;
}

#[cfg(not(feature = "precompiled-shaders"))]
pub(super) struct ShaderCCompiler {
    compiler: shaderc::Compiler,
    options: shaderc::CompileOptions<'static>,
}

#[cfg(not(feature = "precompiled-shaders"))]
impl ShaderCCompiler {
    pub(super) fn new(
        optimize: bool,
        resolve_include: impl Fn(&str, &str) -> Result<String, String> + 'static,
    ) -> Result<Self, RenderError> {
        let compiler = shaderc::Compiler::new()?;
        let mut options = shaderc::CompileOptions::new()?;

        if optimize {
            options.set_optimization_level(shaderc::OptimizationLevel::Performance);
            info!("Enabled optimization by shaderc.");
        } else {
            options.set_optimization_level(shaderc::OptimizationLevel::Zero);
            info!("Disabled optimization by shaderc.");
        }
        options.set_forced_version_profile(430, shaderc::GlslProfile::Core);
        // options.set_generate_debug_info();
        options.set_include_callback(move |name, _, shader_name, _| {
            Ok(shaderc::ResolvedInclude {
                resolved_name: name.to_string(),
                content: resolve_include(name, shader_name)?,
            })
        });

        Ok(Self { compiler, options })
    }
}

#[cfg(not(feature = "precompiled-shaders"))]
impl Compiler for ShaderCCompiler {
    fn create_shader_module(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        stage: ShaderStage,
        name: &str,
    ) -> Result<wgpu::ShaderModule, RenderError> {
        prof_span!(_guard, "create_shader_modules");
        use std::borrow::Cow;

        let file_name = format!("{}.glsl", name);
        let file_name = file_name.as_str();

        let spv = self
            .compiler
            .compile_into_spirv(source, stage.into(), file_name, "main", Some(&self.options))
            .map_err(|e| (file_name, e))?;

        // Uncomment me to dump shaders to files
        //
        // std::fs::create_dir_all("dumpped-shaders").expect("Couldn't create shader
        // dumps folders");
        //
        // let mut file = std::fs::File::create(format!("dumpped-shaders/{}.spv",
        // file_name))     .expect("Couldn't open shader out");
        //
        // use std::io::Write;
        //
        // file.write(spv.as_binary_u8())
        //     .expect("Couldn't write shader out");

        // let label = [file_name, "\n\n", source].concat();

        let descriptor = wgpu::ShaderModuleDescriptor {
            label: Some(file_name),
            source: wgpu::ShaderSource::SpirV(Cow::Borrowed(spv.as_binary())),
        };
        let runtimechecks = wgpu::ShaderRuntimeChecks::unchecked();
        #[expect(unsafe_code)]
        Ok(unsafe { device.create_shader_module_trusted(descriptor, runtimechecks) })
    }
}

pub(super) struct WgpuCompiler {
    reg: regex::Regex,
    resolve_include: Box<dyn Fn(&str, &str) -> Result<String, String> + 'static>,
}

#[cfg(feature = "precompiled-shaders")]
mod baked {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/shaderbin/catalog.rs"));
}

#[cfg(feature = "precompiled-shaders")]
pub(super) struct PrecompiledCompiler {
    includes: WgpuCompiler,
}

#[cfg(feature = "precompiled-shaders")]
impl PrecompiledCompiler {
    pub(super) fn new(
        resolve_include: impl Fn(&str, &str) -> Result<String, String> + 'static,
    ) -> Result<Self, RenderError> {
        Ok(Self {
            includes: WgpuCompiler::new(resolve_include)?,
        })
    }
}

#[cfg(feature = "precompiled-shaders")]
impl Compiler for PrecompiledCompiler {
    fn create_shader_module(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        _stage: ShaderStage,
        name: &str,
    ) -> Result<wgpu::ShaderModule, RenderError> {
        use sha2::{Digest, Sha256};
        let sha256 = |bytes: &[u8]| -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let mut source = source.to_owned();
        for _ in 0..64 {
            let mut failure = None;
            let expanded = self
                .includes
                .reg
                .replace_all(&source, |cap: &regex::Captures| {
                    match (self.includes.resolve_include)(&cap[1], name) {
                        Ok(text) => text.trim_end_matches('\n').to_owned(),
                        Err(error) => {
                            failure = Some(error);
                            String::new()
                        },
                    }
                })
                .into_owned();
            if let Some(error) = failure {
                return Err(RenderError::CustomError(error));
            }
            if expanded == source {
                break;
            }
            source = expanded;
        }
        if self.includes.reg.is_match(&source) {
            return Err(RenderError::CustomError(format!(
                "Unresolved shader includes: {name}"
            )));
        }
        let canonical = source
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let hash = sha256(canonical.as_bytes());
        let (_, _, binary_hash, bytes) = baked::SHADERS
            .iter()
            .find(|(label, source_hash, _, _)| *label == name && *source_hash == hash)
            .ok_or_else(|| {
                RenderError::CustomError(format!(
                    "No precompiled shader for {name} and this rendering configuration; rebake \
                     shaderbin"
                ))
            })?;
        if sha256(bytes) != *binary_hash {
            return Err(RenderError::CustomError(format!(
                "Corrupt precompiled shader: {name}"
            )));
        }
        Ok(device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::util::make_spirv(bytes),
        }))
    }
}

impl WgpuCompiler {
    pub(super) fn new(
        resolve_include: impl Fn(&str, &str) -> Result<String, String> + 'static,
    ) -> Result<Self, RenderError> {
        let reg = regex::Regex::new("(?mR)^#include +<(.+)>$").unwrap();
        Ok(Self {
            reg,
            resolve_include: Box::new(resolve_include),
        })
    }
}

impl Compiler for WgpuCompiler {
    fn create_shader_module(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        stage: ShaderStage,
        name: &str,
    ) -> Result<wgpu::ShaderModule, RenderError> {
        use std::borrow::Cow;

        prof_span!(_guard, "create_shader_modules");

        let label = name;

        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        // replace all `includes` recursivly
        let mut source = Cow::Borrowed(source);
        let source = loop {
            let resolve_includes = self.reg.replace_all(&source, |cap: &regex::Captures| {
                (self.resolve_include)(cap.get(1).unwrap().as_str(), name).unwrap() //TODO unwrap! replace with https://docs.rs/regex/latest/regex/struct.Regex.html#fallibility
            });

            match resolve_includes {
                Cow::Borrowed(source) => break source,
                Cow::Owned(s) => source = Cow::Owned(s),
            }
        };

        let descriptor = wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Glsl {
                shader: Cow::Borrowed(source),
                stage: stage.into(),
                defines: &[],
            },
        };
        let runtimechecks = wgpu::ShaderRuntimeChecks::unchecked();
        #[expect(unsafe_code)]
        let shader = unsafe { device.create_shader_module_trusted(descriptor, runtimechecks) };

        // Pipeline recreation runs inside a Tokio CPU task. This wgpu
        // validation future needs no Tokio reactor; nesting Runtime::block_on
        // here panics whenever a graphics setting rebuilds the shaders.
        if let Some(error) = futures_executor::block_on(error_scope.pop()) {
            Err(RenderError::ShaderWgpuError(label.to_owned(), error))
        } else {
            Ok(shader)
        }
    }
}
