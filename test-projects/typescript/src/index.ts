import { greet, person } from "./greeter.js";
import { legacyName } from "./legacy.js";

const ada = person("Ada", 36);

console.log(greet("world"));
console.log(greet(ada));
console.log(greet(legacyName));

// Kept deliberately unused so `diagnostics` has a warning to report
// (`noUnusedLocals` in tsconfig.json).
const unused = 0;
