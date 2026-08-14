import { cp, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';

const destination=process.argv[2]; if(!destination) throw new Error('usage: node prepare-candidate.mjs DESTINATION');
const source=resolve(new URL('../target-app',import.meta.url).pathname); const target=resolve(destination);
await rm(target,{recursive:true,force:true}); await mkdir(target,{recursive:true}); await cp(source,target,{recursive:true});
console.log(JSON.stringify({candidate:target,source,containsEvaluatorFiles:false}));
