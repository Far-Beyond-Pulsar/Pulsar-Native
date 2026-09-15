# Virtual-geometry CPU mirror ownership

The virtual-geometry pass must not republish its instance records into
SceneDB just to compute CPU cull metadata. SceneDB's cached contract makes
`World` the authoritative component store and exposes generic read queries,
but the current Helio virtual-object API still owns the VG records and
publishes `VgFrameData` to the renderer. Creating a second, pass-specific
SceneDB component set therefore creates two authorities and also introduces
ordering/lifetime work that the pass does not need.

The production-safe boundary is:

- Helio publishes one `VgFrameData::instances` slice and the pass uploads that
  slice to its own GPU instance buffer.
- The pass derives a short-lived `InstanceCullData` projection from the slice
  and the material table. This projection contains only scale/cone-cull and
  opaque/alpha classification, not scene content.
- SceneDB continues to be flushed for components that actually use its mirror;
  VG does not add a pass-specific component or entity lifecycle to that world.

This leaves `helio-core` unaware of the virtual-geometry pass and does not rely
on an unpinned or invented SceneDB API. A future migration can replace the
`VgFrameData` producer with a generic SceneDB CPU projection/iterator once the
VG object/material components and their stable ordering are defined upstream.
Until then, making SceneDB the sole authority for VG is externally blocked by
the missing upstream component/ordering contract, not by CPU cull sorting.
