# Plugin_Cables

Before any change to Cables' rendering, load Helio's
`helio-architecture` skill at
`crates/renderer/helio/.agents/skills/helio-architecture/SKILL.md` from the
Pulsar-Native root. Follow it in order and finish with Design, AAA comparison,
Deleted and Verified sections.

The plugin is not part of `plugins/installed` until its implementation is
ready to ship. Keep all component, GPU contract, feature and editor ownership
inside this plugin; do not add cable-specific behavior to Helio core or the
engine host.
