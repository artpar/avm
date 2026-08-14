import { writeFile } from 'node:fs/promises';
await writeFile('agent-output.json',JSON.stringify({ran:true})); console.log(JSON.stringify({toolCalls:1,modelTokens:10}));
