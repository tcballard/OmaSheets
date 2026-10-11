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
add('math', '=PI()');
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
  '=COUNTIF({"apple","Apricot","berry"},"a*")',
  '=COUNTIF({"a*b","axb","a?b"},"a~*b")',
  '=SUMIF({"apple","Apricot","berry"},"a*",{2,3,4})',
  '=SUMIFS({10,20,30},{"N","S","N"},"N",{1,2,3},">1")',
  '=AVERAGEIFS({10,20,30},{"N","S","N"},"N")',
  '=COUNTIF(Inputs!A1:A5,"")','=COUNTIF(Inputs!A1:A5,">=2")',
  '=SUMIF(Inputs!A1:A5,">=2",Inputs!A1:A5)',
  '=NORM.DIST(1,0,1,TRUE())','=NORM.S.DIST(1,TRUE())',
  '=COVARIANCE.P({1,2,3},{4,5,6})',
]) add('statistics', formula);
for (const name of ['STDEV.S','STDEV.P','VAR.S','VAR.P']) add('statistics', `=${name}(${data})`);
const database = '{"item","cost";"one",10;"two",20;"one",30}';
for (const name of ['DAVERAGE','DMAX','DMIN','DSTDEV']) {
  for (const criteria of ['{"item";"one"}','{"cost";">15"}']) add('statistics', `=${name}(${database},"cost",${criteria})`);
}
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
  '=HYPERLINK("https://example.com","Docs")',
]) add('text', formula);
for (const [y,m,d] of [[2024,2,29],[2025,1,1],[2026,10,10],[2024,13,1],[2024,3,0]]) {
  const date=`DATE(${y},${m},${d})`;
  for (const name of ['YEAR','MONTH','DAY','WEEKDAY']) add('dates', `=${name}(${date})`);
  for (const offset of [-1,0,1]) for (const name of ['EDATE','EOMONTH']) add('dates', `=${name}(${date},${offset})`);
}
for (const [h,m,s] of [[0,0,0],[12,30,45],[23,59,59],[25,61,61],[0,0,59.9],[12,30,0],[23,59,59.9]]) {
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
  '=XNPV(0.1,{-100,110},{45292,45658})','=XIRR({-100,110},{45292,45658})',
  '=FV(0.05,10,-100,-1000,0)','=FV(0.05,10,-100,-1000,1)',
  '=NPER(0.01,-100,2000,500,1)',
  '=IPMT(0.005,12,24,2000,500,0)','=PPMT(0.005,12,24,2000,500,0)',
]) add('finance',formula);
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
  '=CHOOSE(2,1/0,7,NA())','=CHOOSE(1,Inputs!A1,Inputs!A2)',
  '=SUBTOTAL(9,Inputs!A1:A5)','=SUBTOTAL(1,Inputs!A1:A5)',
  '=SUBTOTAL(2,Inputs!A1:A5)','=SUBTOTAL(10,{1,2,3})',
  '=ROW(Inputs!A2)','=COLUMN(Inputs!A2)',
  '=SUM(OFFSET(Inputs!A1,0,0,2,1))','=INDIRECT("A1")',
  '=SUBTOTAL(9,{1,2;3,4})','=SUBTOTAL(7,{1,2,3})','=SUBTOTAL(11,{1,2,3})',
]) add('references',formula);
// A live dependency slice: nested subtotal cells are excluded, ordinary
// formula cells remain included, and SUM provides the independent contrast.
const nestedRow = cases.length + 1;
add('references', '=SUBTOTAL(9,Inputs!A1:A2)');
add('references', '=Inputs!A1');
add('references', `=SUBTOTAL(9,A${nestedRow}:A${nestedRow + 1})`);
add('references', `=SUM(A${nestedRow}:A${nestedRow + 1})`);
add('references', `=SUBTOTAL(9,A${nestedRow})`);
// Error origin must survive real dependencies, nesting and array evaluation.
for (const expression of ['SQRT(-1)', 'VALUE("bad")', '#VALUE!', '#NUM!', '1/0', 'NA()']) {
  const row = cases.length + 1;
  add('errors', `=${expression}`);
  for (const formula of [
    `=ERROR.TYPE(A${row})`, `=ERROR.TYPE(A${row}+1)`,
    `=ERROR.TYPE(SUM(A${row}:A${row}))`, `=ERROR.TYPE(IF(TRUE(),A${row},42))`,
    `=ERROR.TYPE(IFNA(A${row},42))`, `=IFERROR(A${row},42)`,
    `=ISERR(A${row})`, `=ISNA(A${row})`, `=ERROR.TYPE(INDEX(A${row}:A${row},1))`,
    `=SUBTOTAL(2,A${row}:A${row})`, `=SUBTOTAL(3,A${row}:A${row})`,
  ]) add('errors', formula);
}
for (const formula of [
  '=SUMPRODUCT(ISNA(ERROR.TYPE(SQRT({-1,4,-9})))*1)',
  '=SUMPRODUCT(ISERR(SQRT({-1,4,-9}))*1)',
  '=ERROR.TYPE(SQRT(#NUM!))', '=ERROR.TYPE(VALUE(#VALUE!))',
  '=ERROR.TYPE(SQRT(1/0))', '=ERROR.TYPE(VALUE(NA()))',
]) add('errors', formula);
// Scalar arguments and cells/arrays follow different aggregate coercion rules.
for (const name of ['SUM','AVERAGE','MIN','MAX','PRODUCT','COUNT']) {
  for (const args of ['TRUE(),2', '"3",2', '"bad",2', '{TRUE,2}', '{"3",2}', 'Inputs!A1:A5']) {
    add('coercion', `=${name}(${args})`);
  }
}
for (const name of ['COUNT','COUNTA']) {
  for (const args of ['NA(),2', '#VALUE!,2', '{#N/A,2}', 'SQRT(-1),2', '"bad",2', 'TRUE(),2']) {
    add('coercion', `=${name}(${args})`);
  }
}
for (const name of ['SUM','AVERAGE','MIN','MAX','PRODUCT']) {
  add('coercion', `=ERROR.TYPE(${name}("bad",2))`);
}
for (const formula of [
  '=AVERAGE(IF({1,2,3,4,5}>3,{1,2,3,4,5}))',
  '=COUNT(IF({10,0,30,0,50}>0,{10,0,30,0,50}))',
]) add('coercion', formula);
for (const formula of [
  '=AVERAGE(IF({0,1},{10,20}))',
  '=AVERAGE(IF({0,1},{10,20},FALSE()))',
  '=AVERAGE(IF({0,1},{10,20},0))',
  '=COUNTA(IF({0,1},{10,20}))',
  '=SUMPRODUCT(ISBLANK(IF({0,1},{10,20}))*1)',
  '=AVERAGE(TRANSPOSE(IF({0,1},{10,20})))',
  '=AVERAGE(INDEX(TRANSPOSE(IF({0,1},{10,20})),0))',
  '=AVERAGE(IF({0,1},{10,20})*1)',
  '=COUNT(IF({0,1},{10,20},FALSE()))',
  '=AVERAGE(IF({1,1},IF({0,1},{10,20}),0))',
]) add('coercion', formula);
// Preserve boolean formula results through real cell dependencies, rather than
// relying on XLSX's normalization of literal boolean input cells to numbers.
const logicalRow = cases.length + 1;
add('aggregate-edges', '=TRUE()');
add('aggregate-edges', '=FALSE()');
for (const name of ['SUM','AVERAGE','MIN','MAX','PRODUCT','COUNT','COUNTA','SUMSQ','MEDIAN','STDEV','VAR','AVERAGEA']) {
  for (const arg of [`A${logicalRow}`, `A${logicalRow}:A${logicalRow + 1}`]) {
    add('aggregate-edges', `=${name}(${arg},2)`);
  }
}
for (const name of ['SUM','AVERAGE','MIN','MAX','PRODUCT','COUNT','COUNTA','SUMSQ','MEDIAN','STDEV','STDEVP','VAR','VARP','AVERAGEA','GCD','LCM']) {
  // Calc canonicalizes A5:A5 to A5 in XLSX even where the cached result differs
  // (LCM: empty range = 1, empty scalar reference = 0). Use a real rectangle
  // for the imported-range probe; the engine separately tests both expressions.
  add('aggregate-edges', `=${name}(Inputs!A5:${name === 'LCM' ? 'A6' : 'A5'})`);
}
for (const name of ['SUMSQ','MEDIAN','STDEV','STDEVP','VAR','VARP','AVERAGEA','GCD','LCM']) {
  for (const args of ['TRUE(),2', '"3",2', '"bad",2', '{TRUE,2}', '{"3",2}', 'IF({0,1},{10,20})']) {
    add('aggregate-edges', `=${name}(${args})`);
  }
}
for (const formula of [
  '=PRODUCT({"text"})', '=PRODUCT(IF({0,0},{10,20}))',
  '=COUNTBLANK(IF({0,1},{10,20}))', '=COUNTBLANK(IF({0,1},{10,20},""))',
  '=SUM(1E16,1,-1E16)', '=SUM({1E16,1,-1E16})',
  '=SUM(1E16,-1E16,1)', '=SUM({1E16,-1E16,1})',
  '=AVERAGE({1E16,1,-1E16})', '=SUM(0.1,0.2,-0.3)',
  '=STDEV({10000000001,10000000002,10000000003})',
  '=VAR({10000000001,10000000002,10000000003})',
]) add('aggregate-edges', formula);
for (const expr of [`MIN(A${logicalRow},2)`, `MIN(2,A${logicalRow})`, `MIN(A${logicalRow}:A${logicalRow + 1},2)`, `MAX(A${logicalRow},0)`]) {
  add('aggregate-edges', `=${expr}`);
  add('aggregate-edges', `=ISNUMBER(${expr})`);
  add('aggregate-edges', `=ISLOGICAL(${expr})`);
}
for (const name of ['GCD','LCM']) {
  for (const arg of ['"bad",2', '{"3",2}', '-1,2', '{2,TRUE}', '{2,FALSE}', '{FALSE,2}', 'Inputs!A3', 'Inputs!A3:A5', 'Inputs!A5', 'IF({0,1},{10,20},0)']) {
    add('aggregate-edges', `=${name}(${arg})`);
  }
  for (const arg of ['"bad",2', '{"3",2}', '-1,2', 'IF({0,1},{10,20})']) {
    add('aggregate-edges', `=ERROR.TYPE(${name}(${arg}))`);
  }
}
for (const arg of ['TRUE()', 'FALSE()', `A${logicalRow}`, `A${logicalRow + 1}`]) {
  add('aggregate-edges', `=ISNUMBER(${arg})`);
}
add('aggregate-edges', '=SUMPRODUCT(ISNUMBER({TRUE,FALSE,1,"x"})*1)');
// Statistical families: valid, empty, text, logical, domain and error-origin cases.
const statisticalNames = ['MINA','MAXA','STDEVA','STDEVPA','VARA','VARPA','AVEDEV','DEVSQ','GEOMEAN','HARMEAN','MODE','MODE.SNGL','SKEW','SKEW.P','KURT'];
for (const name of statisticalNames) {
  for (const args of [data, '{0,2,4}', '{-5,-2,1,7}', '{2,2,2,2}', '{1}', 'Inputs!A5:A6', 'Inputs!A1:A5',
      'TRUE(),2,4', '"3",2,4', '"bad",2,4', '{TRUE,FALSE,2,4}', '{"3",2,4}', '{"",2,4}',
      'IF({0,1,1},{10,20,30})', 'A' + logicalRow + ':A' + (logicalRow + 1) + ',2,4']) {
    add('statistics-expanded', '=' + name + '(' + args + ')');
  }
  for (const args of ['"bad",2', '{0,0,0,0}', 'Inputs!A5:A6', 'NA(),2']) {
    add('statistics-expanded', '=ERROR.TYPE(' + name + '(' + args + '))');
  }
}
for (const name of ['LARGE','SMALL','PERCENTILE.INC','PERCENTILE.EXC','QUARTILE.INC','QUARTILE.EXC']) {
  const selector = name.startsWith('PERCENTILE') ? '0.5' : '1';
  for (const arg of ['{TRUE,FALSE,2,4}', '{"3",2,4}', 'IF({0,1,1},{10,20,30})', 'Inputs!A5:A6', '"3"', 'A' + logicalRow + ':A' + (logicalRow + 1)]) {
    add('statistics-expanded', '=' + name + '(' + arg + ',' + selector + ')');
  }
  for (const k of [-0.1, 0, 1, 5]) {
    add('statistics-expanded', '=' + name + '({1,2,3,4},' + k + ')');
    add('statistics-expanded', '=ERROR.TYPE(' + name + '({1,2,3,4},' + k + '))');
  }
}
for (const name of ['RANK','RANK.EQ','RANK.AVG']) {
  for (const arg of ['{TRUE,FALSE,2,4}', 'IF({0,1,1},{10,1,30})', 'Inputs!A5:A6', '"3"', 'A' + logicalRow + ':A' + (logicalRow + 1)]) {
    add('statistics-expanded', '=' + name + '(1,' + arg + ')');
  }
  add('statistics-expanded', '=' + name + '(0.1+0.2,{0.3,1})');
  add('statistics-expanded', '=' + name + '(2,{1,2,2,3},1)');
}
for (const name of ['PERCENTRANK','PERCENTRANK.INC','PERCENTRANK.EXC']) {
  for (const n of [0,1,2,3,8,16,17]) add('statistics-expanded', '=' + name + '(' + data + ',' + n + ')');
  for (const sig of [0,1,2,4,5.9]) add('statistics-expanded', '=' + name + '(' + data + ',3,' + sig + ')');
  for (const arg of ['{2}', '{2,2,2}', 'Inputs!A5:A6', '"3"', 'IF({0,1},{10,2})']) add('statistics-expanded', '=' + name + '(' + arg + ',2)');
}
for (const fraction of [-0.1,0,0.1,0.25,0.5,0.9,1]) add('statistics-expanded', '=TRIMMEAN(' + data + ',' + fraction + ')');
for (const arg of ['Inputs!A5:A6','{TRUE,FALSE,2,4}','"3"','IF({0,1,1},{10,20,30})']) add('statistics-expanded', '=TRIMMEAN(' + arg + ',0.5)');
for (const args of ['1,0,1','10,5,2','1,0,0','1,0,-1','TRUE(),FALSE(),2','"3",0,1']) {
  add('statistics-expanded', '=STANDARDIZE(' + args + ')');
  add('statistics-expanded', '=ERROR.TYPE(STANDARDIZE(' + args + '))');
}
for (const name of ['SLOPE','INTERCEPT','RSQ','STEYX','PEARSON','FORECAST','FORECAST.LINEAR']) {
  for (const args of ['{2,4,6,8},{1,2,3,4}', '{1,5,7,8},{2,3,4,6}', '{4,4,4},{1,2,3}', '{1,2,3},{4,4,4}',
    '{1},{2}', '{1,2},{3,4}', '{1,2,3},{3,4}', '{TRUE,2,4},{1,2,3}', '{"x",2,4},{1,2,3}',
    'IF({0,1,1},{1,2,4}),{1,2,3}', '{1,#N/A,3},{1,2,3}', 'Inputs!A5:A6,Inputs!A5:A6']) {
    add('statistics-expanded', '=' + name + '(' + (name.startsWith('FORECAST') ? '5,' : '') + args + ')');
  }
}
for (const expr of ['MODE({4,4,2,2})','MODE.SNGL({4,4,2,2})','MODE.SNGL({4,2;2,4})',
  'GEOMEAN(0,NA())','GEOMEAN(NA(),0)','GEOMEAN({0,-1})','GEOMEAN({-1,0})','GEOMEAN({0,#N/A})']) {
  add('statistics-expanded', '=' + expr);
}
// Distinguish direct, reference and matrix errors before changing origins.
const statisticalErrorRow = cases.length + 1;
add('statistics-origins', '=NA()');
for (const name of ['AVEDEV','GEOMEAN','HARMEAN','SKEW','SKEW.P','KURT']) {
  for (const args of ['NA(),2','2,NA()','{1,#N/A,3}','A' + statisticalErrorRow + ',2','#NUM!,2','1/0,2']) {
    add('statistics-origins', '=' + name + '(' + args + ')');
    add('statistics-origins', '=ERROR.TYPE(' + name + '(' + args + '))');
  }
}
for (const args of ['0,NA()','NA(),0','{0,#N/A}','{#N/A,0}','0,"bad"','"bad",0','0,-1','-1,0']) {
  add('statistics-origins', '=GEOMEAN(' + args + ')');
  add('statistics-origins', '=ERROR.TYPE(GEOMEAN(' + args + '))');
}
for (const name of ['SLOPE','INTERCEPT','RSQ','STEYX','PEARSON','CORREL','COVARIANCE.P','COVARIANCE.S']) {
  for (const args of ['{1,2,3},{3,4}','{1},{2}','{1;2},{3,4}','{2,2,2},{1,2,3}']) {
    add('statistics-origins', '=' + name + '(' + args + ')');
    add('statistics-origins', '=ERROR.TYPE(' + name + '(' + args + '))');
  }
}
for (const arg of ['"bad"','" 3 "','Inputs!A3','NA()']) add('statistics-origins', '=STANDARDIZE(' + arg + ',0,1)');
for (const expr of ['MODE.MULT({4,4,2,2})','SUM(MODE.MULT({4,4,2,2}))','INDEX(MODE.MULT({4,4,2,2}),2)',
  'SUM(MODE.MULT({1,2,3}))','SUM(MODE.MULT({4,2;2,4}))','SUM(MODE.MULT({TRUE,1,FALSE,0,2}))']) add('statistics-arrays', '=' + expr);
for (const name of ['LARGE','SMALL']) for (const ranks of ['{1,2,3}','{1;2;3}','{1,3;2,4}','{1.2,2.8}',
    '{0,2}','{5,2}','{TRUE,2}','{"1",2}','{"bad",2}','{#N/A,2}']) {
  add('statistics-arrays', '=SUM(' + name + '({1,2,3,4},' + ranks + '))');
  add('statistics-arrays', '=ERROR.TYPE(SUM(' + name + '({1,2,3,4},' + ranks + ')))');
}
for (const name of ['MINA','MAXA','STDEVA','STDEVPA','VARA','VARPA','DEVSQ','MODE.SNGL','GEOMEAN']) {
  for (const arg of ['INDEX(Inputs!A5:A6,0)','INDEX(Inputs!A1:A5,0)','INDEX(IF({0,1,1},{10,20,30}),1,0)']) {
    add('statistics-references', '=' + name + '(' + arg + ')');
  }
}
for (const expr of ['INDEX(SMALL({1,2,3,4},{1,2}),1,2)','SUM(TRANSPOSE(MODE.MULT({4,4,2,2})))','MINA(IF({0,0},{10,20}))','VARA(IF({0,0},{10,20}))']) add('statistics-references', '=' + expr);
// Cell publication and nested inspection must distinguish pending errors.
for (const expr of ['AVEDEV(NA(),2)','AVEDEV(1/0,2)','GEOMEAN({0,#N/A})','GEOMEAN(NA(),0)','SUM(LARGE({1,2,3},{#N/A,2}))']) {
  const row = cases.length + 1;
  add('statistics-pending', '=' + expr);
  for (const inner of [expr, 'A' + row]) {
    for (const inspection of ['ERROR.TYPE','ISNA','ISERR','ISERROR']) add('statistics-pending', '=' + inspection + '(' + inner + ')');
    for (const handler of ['IFNA','IFERROR']) add('statistics-pending', '=' + handler + '(' + inner + ',42)');
  }
  add('statistics-pending', '=ERROR.TYPE((' + expr + ')+0)');
}
for (const error of ['#N/A','#DIV/0!','#NUM!','#VALUE!','#REF!','#NAME?','#NULL!']) {
  for (const expr of ['AVEDEV(' + error + ',2)', 'SUM(SMALL({1,2,3},{' + error + ',2}))']) {
    add('statistics-pending', '=' + expr);
    add('statistics-pending', '=ERROR.TYPE(' + expr + ')');
  }
}
for (const code of [2,3]) add('statistics-arrays', '=SUBTOTAL(' + code + ',IF({TRUE,FALSE},NA(),{0,2}))');
// Calc's FODS parser rejects literal error members in inline arrays. Build
// equivalent typed arrays with IF instead of measuring a compilation error.
// Preserve the original expression for audit; no case is omitted from the gate.
function validErrorArrays(formula) {
  let output = '', start = 0, quoted = false;
  for (let i = 0; i < formula.length; i++) {
    if (formula[i] === '"') {
      if (quoted && formula[i + 1] === '"') { i++; continue; }
      quoted = !quoted;
    }
    if (quoted || formula[i] !== '{') continue;
    let end = i + 1, quote = false;
    for (; end < formula.length; end++) {
      if (formula[end] === '"') {
        if (quote && formula[end + 1] === '"') { end++; continue; }
        quote = !quote;
      } else if (!quote && formula[end] === '}') break;
    }
    if (end === formula.length) throw new Error('unterminated array fixture');
    const body = formula.slice(i + 1, end), tokens = [], separators = [];
    let from = 0; quote = false;
    for (let j = 0; j <= body.length; j++) {
      if (body[j] === '"') {
        if (quote && body[j + 1] === '"') { j++; continue; }
        quote = !quote;
      }
      if (j === body.length || (!quote && ',;'.includes(body[j]))) {
        tokens.push(body.slice(from, j));
        if (j < body.length) separators.push(body[j]);
        from = j + 1;
      }
    }
    const errors = [...new Set(tokens.map(t => t.trim()).filter(t => /^#(?:N\/A|DIV\/0!|NUM!|VALUE!|REF!|NAME\?|NULL!)$/.test(t)))];
    const array = values => '{' + values.map((v, j) => v + (separators[j] ?? '')).join('') + '}';
    let replacement = array(tokens.map(t => errors.includes(t.trim()) ? '0' : t));
    for (const error of errors) replacement = 'IF(' + array(tokens.map(t => t.trim() === error ? 'TRUE' : 'FALSE')) + ',' + error + ',' + replacement + ')';
    output += formula.slice(start, i) + (errors.length ? replacement : formula.slice(i, end + 1));
    start = end + 1; i = end;
  }
  return output + formula.slice(start);
}
for (const entry of cases) {
  const valid = validErrorArrays(entry.formula);
  if (valid !== entry.formula) {
    entry.originalFormula = entry.formula;
    entry.formula = valid;
  }
}
writeFileSync('tests/calc-parity/common.json',JSON.stringify(cases,null,2)+'\n');
const counts={};for(const c of cases) counts[c.group]=(counts[c.group]??0)+1;
console.log(JSON.stringify({cases:cases.length,groups:counts}));
