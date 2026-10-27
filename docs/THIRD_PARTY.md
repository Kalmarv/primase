# Third-party code in this repository

Code that is derived from another project's source (as opposed to a dependency in `Cargo.lock`, which carries its own
licence) is listed here with where it comes from, the licence it was published under, and where it lives in the
workspace. Each such file keeps the licence text in its header.

## Microsoft Direct3D 11 reference tessellator

- Used in: `https://github.com/Kalmarv/iridia/blob/main/iridia-exec/src/tessellator.rs`, a port to Rust of `CHWTessellator` (the fixed-function
  tessellator: factor processing, point placement, ring and transition stitching).
- Source: `tessellator.cpp` and `tessellator.hpp` ("Microsoft D3D11 Fixed Function Tessellator Reference - May 7, 2012",
  amar.patel@microsoft.com), published with the Direct3D 11.3 Functional Specification in
  <https://github.com/microsoft/DirectX-Specs>, folder `d3d/archive/images/d3d11/`
  (<https://microsoft.github.io/DirectX-Specs/d3d/archive/images/d3d11/tessellator.cpp>). `CHLSLTessellator` of the same
  file (the factor preconditioning the HLSL compiler generates into hull programs) is not ported: the hull programs
  this code runs carry that arithmetic themselves.
- Licence: the MIT License, Copyright (c) Microsoft Corporation: the `DirectX-Specs` repository states that "any code in
  the repository" is under the MIT License (its `LICENSE-CODE`). The two source files carry no licence header of their
  own. The licence text:

```
The MIT License (MIT)
Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

MIT is compatible with this workspace's GPL-3.0-or-later.
