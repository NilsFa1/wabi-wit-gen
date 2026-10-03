import { DemoGreeter } from "./sdk/ts/demo_greeter.ts";
import { GreetException } from "./sdk/ts/types.ts";

using greeter = new DemoGreeter("Hallo");
const g = await greeter.greet("Nils");
console.log(`${g.text} (${g.count})`);
try {
  await greeter.greet("");
} catch (e) {
  console.log("Fehler:", e instanceof GreetException ? e.error : e);
}
