# Offline Voxygen shaders

Rebuild from the Veloren root with `python3 voxygen/tools/bake_shaders.py`.
Verify source and binary hashes without a compiler with
`python3 voxygen/tools/bake_shaders.py --verify`.

The 82 SPIR-V artifacts cover all 41 active shader stages with
`RenderMode::default()` settings, both with shadow-map views and with the
existing cheap-shadow fallback. Other settings and experimental shaders
require additional baked variants. The bake uses the existing TRUEOS GLSL
frontend lane: glslc, Vulkan 1.1, forced GLSL 430 core, performance optimization.
The 440-to-430 compiler warnings match Voxygen's existing shaderc policy.

Enable Voxygen's `precompiled-shaders` feature to embed these binaries and
bypass runtime GLSL compilation. The loader hashes the resolved GLSL and
checks the binary hash before using wgpu's validated SPIR-V module API.
Unknown configurations and stale source changes fail instead of invoking
a runtime shader compiler. This feature does not remove the shaderc build
dependency or port the client.

These are intermediate SPIR-V binaries, **not admitted TRUEOS Intel EU
executables**. The local wgpu checkout has no TRUEOS backend. Native baking
and execution admission require concrete pipeline layouts, vertex fetch,
descriptor mappings, URB/SBE and pixel payload state, and a corresponding
runtime consumer. The existing fixed-pipeline Mesa bake cannot grant that
contract to arbitrary Voxygen stages. `native_execution_admitted` remains
false in the manifest until those contracts are implemented and verified.
