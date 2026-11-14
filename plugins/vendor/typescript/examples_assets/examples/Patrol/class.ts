// Engine types: Vec3 and a per-frame tick.
//
// Engine types are used through their natives, `Vec3.new_(x, y, z)` and
// methods such as `v.add(w)`; their fields read as `v.x`.
export default class Patrol extends ScriptClass {
    speed: number = 2.5;
    position: Vec3 = Vec3.new_(0, 0, 0);
    direction: Vec3 = Vec3.new_(1, 0, 0);
    elapsed: number = 0.0;
    label: string = "patrol";

    // Called every frame with the frame time in seconds.
    tick(delta: number): void {
        this.elapsed += delta;
        let step: Vec3 = Vec3.new_(
            this.direction.x * this.speed * delta,
            0,
            this.direction.z * this.speed * delta
        );
        this.position = this.position.add(step);

        // Turn around every four seconds.
        if (this.elapsed >= 4.0) {
            this.elapsed = 0.0;
            this.direction = Vec3.new_(0 - this.direction.x, 0, 0 - this.direction.z);
            this.label = "patrol (turned)";
        }
    }

    // How far along the x axis it has walked.
    distanceX(): number {
        return this.position.x;
    }
}
