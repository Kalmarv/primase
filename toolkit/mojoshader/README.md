# MojoShader GLSL lifting

`lift` reads the GLSL dialect MojoShader writes when it translates Shader Model 2 and 3 programs for OpenGL, and builds a
`primase_shader::Program` from it. It is not a GLSL front end: it reads exactly what that translation writes and refuses
anything else by name (`Error`, a `primase_shader::Refusal`).

## The dialect

- `#ifdef GL_ES` precision blocks (skipped), `#extension GL_OES_standard_derivatives`, and `#define` aliases of registers
  (expanded);
- `uniform vec4 <stage>_uniforms_vec4[N]` (the constants, a uniform buffer of `vec4<u32>` rows read as floats; an index
  past the array reads zero), `uniform sampler2D`, `attribute vec4` (vertex), `varying vec4` (`vFrontColor`,
  `vBackColor`, `vFogFactor`, `vTexCoord<n>`), `const vec4` and `const ivec4` constants, `vec4` and `ivec4` registers;
- `main`: assignments to swizzled registers, `if`/`else`, `rep` loops of constant count, `discard`, `?:`, relative
  addressing through an `ivec4` register, `texture2D`, `dFdx`/`dFdy`, `any`, `lessThan`, constructors and the
  math functions the translation emits.

## What is dropped

MojoShader ends every vertex program with three statements that fix the clip space for OpenGL (`posFixup`). They are
the API's, not the program's: they are dropped when they are exactly those three, the program keeps Direct3D's clip
space (`Conventions::D3D`), and the program's notes say so. `posFixup` anywhere else is refused.

## What is refused

A name declared twice, a number that is not finite, an operation on vectors of different widths (`vec2 + vec3`, the
branches of `?:`, the arguments of a function) and any other thing the dialect does not have are refused by name with
the line. The caller's constants that lie past the uniform rows the program declares are left out and the ones that
reach past them are cut; the program's notes count both.

## What the caller states

`Hints` names what the text does not say: the constants' layout, each sampler's slot and the name of what is bound to
it, each attribute's semantic and index.
