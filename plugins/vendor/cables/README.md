# Cables plugin

`pulsar.cables` is linked by `plugins/installed`. It contributes authored
cable/connector components, the cable definition GPU rows, a per-frame
connector pairing, and a Helio render-graph feature. The design and ownership
boundary is recorded in [DESIGN.md](DESIGN.md).

A cable is a `CableComponent` (appearance and simulation, one nonzero ID) and
two or more `CableConnectorComponent`s with the same ID. Each frame the plugin
pairs the connectors that share an ID, as portals pair by Link ID (`n`
connectors make `n * (n - 1) / 2` cables), and the feature draws each pair as
a camera-facing ribbon (2D) or a round tube with bundle lanes (3D), with a
sagged span and animated wind sway. An ID with connectors but no enabled
definition is logged once as a warning.

Debug views (Cables category): *Cable Diagnostics* shows what each stage
found (definition rows, texture table, strands published) and projects every
cable end; *Cable Solid* paints the geometry flat magenta.

`reset_simulation()` increments a revision in the GPU row; persistent
reset-driven physics, using `cylinder_mesh` for the 3D profile, and a dedicated
editor reset action remain to be completed.
