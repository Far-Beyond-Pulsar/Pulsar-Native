// A door that opens, stays open for a while, then closes again.
//
// Fields are typed and initialised with literals. `int` is the engine's
// 64-bit integer; `number` is a float. The two never mix silently, convert
// with `x as number` or `x as int`.
export default class Door extends ScriptClass {
    isOpen: boolean = false;
    openings: int = 0;       // how many times the door has been opened
    openSeconds: number = 2.0;

    // `async` methods may suspend. `await wait(s)` waits `s` seconds of game
    // time, so it pauses with the game and survives hot reload.
    async open(): Promise<void> {
        if (this.isOpen) {
            return;          // already open: nothing to do
        }
        this.isOpen = true;
        this.openings += 1;
        await wait(this.openSeconds);
        this.close();
    }

    close(): void {
        this.isOpen = false;
    }

    // A plain method that returns a value.
    timesOpened(): int {
        return this.openings;
    }
}
