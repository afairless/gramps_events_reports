# Vendored htmx

`htmx.min.js` is the official single-file distribution of **htmx 2.0.4**,
downloaded from https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js and
committed verbatim (byte-identical, 50917 bytes).

It is the only non-Rust artifact in the build (plan §6.4 / D8 acceptance:
"htmx is the only vendored non-Rust artifact (a static file)").

License: htmx is distributed under the BSD 3-Clause license
(https://github.com/bigskysoftware/htmx/blob/master/LICENSE).

It is compiled into the binary via `include_str!` and served at
`/static/htmx.min.js`, so the server ships with no static files on disk.