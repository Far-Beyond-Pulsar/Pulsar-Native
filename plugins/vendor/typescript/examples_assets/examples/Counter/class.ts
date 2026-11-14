// Control flow and numbers.
export default class Counter extends ScriptClass {
    total: int = 0;
    average: number = 0.0;

    // Sum 1..=n with a `for` loop.
    sumTo(n: int): int {
        let sum: int = 0;
        for (let i: int = 1; i <= n; i++) {
            sum += i;
        }
        this.total = sum;
        return sum;
    }

    // Sum the even numbers up to n, skipping the odd ones and stopping early
    // once the running sum passes a limit.
    sumEvens(n: int, limit: int): int {
        let sum: int = 0;
        let i: int = 0;
        while (i < n) {
            i += 1;
            if (i % 2 == 1) {
                continue;
            }
            sum += i;
            if (sum > limit) {
                break;
            }
        }
        return sum;
    }

    // `int` and `number` are different types: convert explicitly.
    averageOf(count: int): number {
        if (count == 0) {
            return 0.0;
        }
        this.average = (this.total as number) / (count as number);
        return this.average;
    }

    // `?:` and `&&` / `||` work as in TypeScript.
    clamp(value: int, low: int, high: int): int {
        return value < low ? low : (value > high ? high : value);
    }
}
