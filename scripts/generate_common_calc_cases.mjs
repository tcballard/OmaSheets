// Deterministic test inputs; expected results come from live Calc, not this file.
import { readFileSync, writeFileSync } from 'node:fs';
const cases = [];
const add = (group, formula) => cases.push({ id: `${group}-${String(cases.length + 1).padStart(4, '0')}`, group, formula });
for (const formula of JSON.parse(readFileSync('tests/calc-parity/information.json', 'utf8'))) add('information', formula);
const unary = {
  ABS: [-3.5, 0, 4.5], INT: [-3.5,0,4.5], SIGN: [-3.5,0,4.5], SQRT: [0,0.25,2,100],
  EXP: [-2,0,0.5,3], LN: [0.01,1,2,100], LOG10: [0.01,1,10,100],
  SIN: [-3,-1,0,0.5,2], COS: [-3,-1,0,0.5,2], TAN: [-2,-0.5,0,0.5,2],
  ASIN: [-1,-0.5,0,0.5,1], ACOS: [-1,-0.5,0,0.5,1], ATAN: [-10,-1,0,1,10],
  SINH: [-3,-1,0,1,3], COSH: [-3,-1,0,1,3], TANH: [-3,-1,0,1,3],
  ASINH: [-10,-1,0,1,10], ACOSH: [1,1.5,2,10], ATANH: [-0.9,-0.5,0,0.5,0.9],
  DEGREES: [-3.141592653589793,0,1,3.141592653589793], RADIANS: [-180,0,30,90,360],
  EVEN: [-5.3,-2,-0.1,0,0.1,2,5.3], ODD: [-5.3,-2,-0.1,0,0.1,2,5.3],
  FACT: [0,1,5,10,5.8], FACTDOUBLE: [0,1,2,7,8], ISEVEN: [-3.9,-2,0,2.9,3], ISODD: [-3.9,-2,0,2.9,3],
};
for (const [name, inputs] of Object.entries(unary)) for (const n of inputs) add('math', `=${name}(${n})`);
for (const n of [-5.55,-2.5,0,2.5,5.55]) for (const precision of [0,1,2]) {
  for (const name of ['ROUND','ROUNDUP','ROUNDDOWN','TRUNC']) add('rounding', `=${name}(${n},${precision})`);
}
for (const [name, pairs] of Object.entries({
  MOD: [[-5,3],[5,-3],[5,3],[-5,-3],[0,3]], QUOTIENT: [[-5,3],[5,-3],[5,3],[0,3]],
  POWER: [[2,3],[2,-3],[9,0.5],[0,2],[-2,3]], MROUND: [[10,3],[-10,-3],[1.3,0.2],[0,2],[10,0]],
  ATAN2: [[1,1],[-1,1],[-1,-1],[1,-1],[0,1],[1,0],[0,0]],
  COMBIN: [[5,2],[10,3],[5,0],[5,5],[5.9,2.1]], COMBINA: [[3,2],[10,3],[5,0],[1,5]],
  GCD: [[12,18],[0,18],[7,11],[12.5,18.9]], LCM: [[12,18],[0,18],[7,11],[12.5,18.9]],
  LOG: [[8,2],[100,10],[0.5,2]], CEILING: [[5.1,2],[-5.1,-2],[0,2]], FLOOR: [[5.1,2],[-5.1,-2],[0,2]],
})) for (const pair of pairs) add('math', `=${name}(${pair})`);
const data = '{1,2,2,4,8,16}';
for (const name of ['SUM','AVERAGE','MIN','MAX','COUNT','COUNTA','PRODUCT','MEDIAN','STDEV','STDEVP','VAR','VARP','SUMSQ','AVERAGEA']) {
  for (const values of [data,'{0,2,4}','{-5,-2,1,7}']) add('statistics', `=${name}(${values})`);
}
for (const name of ['LARGE','SMALL']) for (const k of [1,2,3.2,6]) add('statistics', `=${name}(${data},${k})`);
for (const name of ['PERCENTILE','PERCENTILE.INC','PERCENTILE.EXC']) for (const p of [0.2,0.5,0.8]) add('statistics', `=${name}(${data},${p})`);
for (const name of ['QUARTILE','QUARTILE.INC','QUARTILE.EXC']) for (const p of [1,2,3]) add('statistics', `=${name}(${data},${p})`);
for (const name of ['RANK','RANK.AVG']) for (const n of [1,2,4,16]) for (const order of [0,1]) add('statistics', `=${name}(${n},${data},${order})`);
for (const formula of [
  '=COUNTBLANK({1,"",2,"x"})','=SUMPRODUCT({1,2,3},{4,5,6})','=SUMPRODUCT(({1,2,3}>1)*{4,5,6})',
  '=CORREL({1,2,3,4},{2,4,6,8})','=COVAR({1,2,3},{4,5,6})','=COVARIANCE.S({1,2,3},{4,5,6})',
  '=NORMDIST(0,0,1,TRUE())','=NORMDIST(1,0,1,FALSE())','=NORMSDIST(1)',
  '=COUNTIF({1,2,2,4},2)','=SUMIF({1,2,2,4},">1")','=AVERAGEIF({1,2,2,4},">1")',
  '=COUNTIFS({1,2,2,4},">1",{1,2,2,4},"<4")','=SUMIFS({1,2,2,4},{1,2,2,4},">1")',
  '=AVERAGEIFS({1,2,2,4},{1,2,2,4},">1")',
]) add('statistics', formula);
for (const text of ['"Hello WORLD"','"two words"','""','"éclair"']) for (const name of ['LEN','UPPER','LOWER','PROPER','CLEAN']) add('text', `=${name}(${text})`);
for (const formula of [
  '=LEFT("abcdef",3)','=RIGHT("abcdef",3)','=MID("abcdef",2,3)','=LEFT("abc",0)',
  '=TRIM("  two   words  ")','=CONCAT("a","b","c")','=CONCATENATE("a",2,"c")',
  '=TEXTJOIN(",",TRUE(),{"a","","b"})','=TEXTJOIN("-",FALSE(),{"a","","b"})',
  '=VALUE("123.5")','=EXACT("a","A")','=EXACT("abc","abc")','=FIND("b","abcabc")',
  '=FIND("b","abcabc",3)','=REPT("ab",3)','=REPT("x",0)',
  '=SEARCH("B","abcABC")','=SEARCH("b","abcABC",3)','=SEARCH("a?c","xxAbczz")',
  '=SEARCH("a*c","xxAbbbCzz")','=SEARCH("~*","ab*cd")','=SEARCH("","abc",2)',
  '=SUBSTITUTE("one-one-one","one","two")','=SUBSTITUTE("one-one-one","one","two",2)',
  '=SUBSTITUTE("abc","","x")','=SUBSTITUTE("abc","z","x",1)',
  '=REPLACE("abcdef",2,3,"X")','=REPLACE("abc",5,3,"X")','=REPLACE("éclair",2,2,"X")',
  '=TEXT(1234.5,"0.00")','=TEXT(0.25,"0%")',
]) add('text', formula);
for (const [y,m,d] of [[2024,2,29],[2025,1,1],[2026,10,10],[2024,13,1],[2024,3,0]]) {
  const date=`DATE(${y},${m},${d})`;
  for (const name of ['YEAR','MONTH','DAY','WEEKDAY']) add('dates', `=${name}(${date})`);
  for (const offset of [-1,0,1]) for (const name of ['EDATE','EOMONTH']) add('dates', `=${name}(${date},${offset})`);
}
for (const [h,m,s] of [[0,0,0],[12,30,45],[23,59,59],[25,61,61],[0,0,59.9]]) {
  const time=`TIME(${h},${m},${s})`;
  add('dates',`=${time}`);
  for (const name of ['HOUR','MINUTE','SECOND']) add('dates',`=${name}(${time})`);
}
for (const formula of [
  '=DAYS(DATE(2024,3,1),DATE(2024,2,28))','=DAYS(DATE(2025,1,1),DATE(2024,1,1))',
  '=DAYS(10.5,2.25)',
  '=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,12))','=WORKDAY(DATE(2024,1,1),10)',
  '=YEARFRAC(DATE(2024,1,1),DATE(2025,1,1),3)','=DAYS360(DATE(2024,1,1),DATE(2025,1,1))',
]) add('dates',formula);
for (const rate of [0,0.005,0.01]) for (const due of [0,1]) {
  for (const name of ['PMT','PV','FV']) add('finance', `=${name}(${rate},24,${name==='PV'?-100:name==='FV'?-100:2000},0,${due})`);
  add('finance',`=NPER(${rate},-100,2000,0,${due})`);
  for (const period of [1,2,24]) for (const name of ['IPMT','PPMT']) add('finance', `=${name}(${rate},${period},24,2000,0,${due})`);
}
for (const formula of [
  '=SLN(10000,1000,5)','=SYD(10000,1000,5,1)','=SYD(10000,1000,5,5)',
  '=NPV(0.1,{-100,40,50,60})','=IRR({-100,40,50,60})','=RRI(5,100,200)',
  '=XNPV(0.1,{-100,110},{DATE(2024,1,1),DATE(2025,1,1)})',
]) if(!formula.includes('{DATE')) add('finance',formula);
for (const formula of [
  '=IF(TRUE(),7,1/0)','=IF(FALSE(),1/0,7)','=AND(TRUE(),TRUE())','=OR(FALSE(),TRUE())',
  '=NOT(FALSE())','=XOR(TRUE(),FALSE())','=XOR(TRUE(),TRUE())','=XOR(1,0,1)',
  '=ISBLANK(Inputs!A5)','=ISNUMBER(Inputs!A1)','=ISTEXT(Inputs!A3)','=ISLOGICAL(Inputs!A4)',
  '=SUM(Inputs!A1:A5)','=COUNTA(Inputs!A1:A5)','=COUNTBLANK(Inputs!A1:A5)',
  '=INDEX({10,20,30},1,2)','=MATCH(20,{10,20,30},0)','=LOOKUP(25,{10,20,30},{1,2,3})',
  '=VLOOKUP(2,{1,10;2,20;3,30},2,FALSE())','=HLOOKUP(2,{1,2,3;10,20,30},2,FALSE())',
  '=XLOOKUP(2,{1,2,3},{10,20,30})','=SUM(INDEX({1,2;3,4},0,2))',
  '=SUM(TRANSPOSE({1,2;3,4}))','=SUM(MMULT({1,2;3,4},{1,0;0,1}))',
  '=SUM(IF({1,2,3}>1,{10,20,30},0))','=SUMPRODUCT(SIN({0,1,2})*1)',
  '=IFERROR(QUOTIENT(2,0),99)','=ISERROR(ASIN(2))','=ISERROR(FACT(-1))',
]) add('references',formula);
writeFileSync('tests/calc-parity/common.json',JSON.stringify(cases,null,2)+'\n');
const counts={};for(const c of cases) counts[c.group]=(counts[c.group]??0)+1;
console.log(JSON.stringify({cases:cases.length,groups:counts}));
