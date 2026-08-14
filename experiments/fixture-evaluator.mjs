import { access } from 'node:fs/promises';
const workspace=process.argv[2]; let functionalDefects=0; try{await access(`${workspace}/agent-output.json`)}catch{functionalDefects=1}
console.log(JSON.stringify({functionalDefects,hiddenDefectsDiscovered:0,userFacingDefects:0,temporalDefects:0,incorrectRequirementInterpretations:0,regressions:0,rework:0,failedAttempts:0,timeMs:1,toolCalls:1,modelTokens:10,productInteractions:0,diagnosisAccuracy:null,humanInterventions:0,catastrophicImplementations:0}));
