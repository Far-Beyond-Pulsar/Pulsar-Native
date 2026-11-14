// Changing a class without losing saved data.
//
// This is the second version of a class whose first version had `hp: int`
// and `name: string`. A field keeps its identity in class.schema.json, so:
//   * `@renamedFrom("hp")` says `health` is the old `hp`, keeping its value;
//   * `shield` is new, and `migrate` fills it from the old data;
//   * `name` was removed, and its value is dropped.
// `migrate` runs once, after the class version rises, and reads old values
// through the `migration` natives.
export default class Hero extends ScriptClass {
    @renamedFrom("hp")
    health: int = 10;
    position: Vec3 = Vec3.new_(1, 2, 3);
    shield: number = 0;

    migrate(from: int): void {
        this.shield = (migration.old_int("hp") as number) / 100;
    }
}
