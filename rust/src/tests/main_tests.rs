// Unit tests of the solver executable (main.rs): the tool is driven in-process through ToporToolMain

use super::*;

// Runs the tool with the given arguments (excluding the name of the executable); returns the exit code (or the uncaught error) and the output
pub fn RunTool(args: &[&str]) -> (Result<i32, String>, String)
{
	let argv: Vec<String> = std::iter::once("intel_sat_solver").chain(args.iter().copied()).map(|s| s.to_string()).collect();
	let mut out: Vec<u8> = vec![];
	let res = ToporToolMain(&argv, &mut out);
	(res, String::from_utf8_lossy(&out).into_owned())
}

pub fn RegressionInstances() -> Vec<String>
{
	let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../regression_instances");
	let mut res: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path().to_string_lossy().into_owned()).filter(|p| p.ends_with(".cnf")).collect();
	res.sort();
	assert!(res.len() > 70);
	res
}

pub fn TmpFile(name: &str) -> String
{
	let dir = std::env::temp_dir().join(format!("intel_sat_solver_tests_{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	dir.join(name).to_string_lossy().into_owned()
}

// The regression instances in every solver mode: the tool verifies every model and every unsatisfiable core by itself;
// the verdicts must be the same in all the modes
fn Regression(part: usize)
{
	for (i, f) in RegressionInstances().iter().enumerate().filter(|(i, _)| i % 4 == part)
	{
		let mut verdicts: Vec<Vec<String>> = vec![];
		for solverMode in ["0", "1", "2"]
		{
			let drat = TmpFile(&format!("regr_{}_{}.drat", i, solverMode));
			let dratParam = if i % 2 == 0 { "/topor_tool/text_drat_file" } else { "/topor_tool/bin_drat_file" };
			// Verifying a core re-solves, so the core cannot be printed afterwards (the tool reports an error then, precisely as the C++ tool does)
			let ucoreParam = if i % 5 < 3 { "/topor_tool/verify_ucore" } else { "/topor_tool/print_ucore" };
			let (res, out) = RunTool(&[f, "/topor_tool/solver_mode", solverMode, "/topor_tool/verify_model", "1", ucoreParam, "1", dratParam, &drat,
				"/topor_tool/drat_sort_every_clause", if i % 3 == 0 { "1" } else { "0" }]);
			let ret = res.unwrap();
			assert!(ret == 10 || ret == 20, "{} mode {}: {}\n{}", f, solverMode, ret, out);
			assert!(!out.contains("ERROR") && !out.contains("rror:"), "{} mode {}:\n{}", f, solverMode, out);
			assert!(std::fs::metadata(&drat).unwrap().len() > 0 || !out.contains("s UNSATISFIABLE"));
			let _ = std::fs::remove_file(&drat);
			verdicts.push(out.lines().filter(|l| l.starts_with("s ")).map(|l| l.to_string()).collect());
			assert!(!verdicts.last().unwrap().is_empty());
		}
		assert_eq!(verdicts[0], verdicts[1], "{}", f);
		assert_eq!(verdicts[0], verdicts[2], "{}", f);
	}
}

#[test]
fn regression_0()
{
	Regression(0);
}

#[test]
fn regression_1()
{
	Regression(1);
}

#[test]
fn regression_2()
{
	Regression(2);
}

#[test]
fn regression_3()
{
	Regression(3);
}

/*
* Targeted tests of the tool: the incremental format, the parameters, the error paths, AllSAT, the archives and the free functions
*/

// Writes the given contents into a temporary file and runs the tool on it with the given parameters
fn RunCnf(name: &str, contents: &[u8], params: &[&str]) -> (Result<i32, String>, String)
{
	let f = TmpFile(name);
	std::fs::write(&f, contents).unwrap();
	let mut args: Vec<&str> = vec![&f];
	args.extend_from_slice(params);
	let r = RunTool(&args);
	let _ = std::fs::remove_file(&f);
	r
}

// The number of the lines of the output, which are equal to the given one
fn CountLines(out: &str, line: &str) -> usize
{
	out.lines().filter(|l| *l == line).count()
}

// The tool must return the given value and print the given string
fn ExpectRet(name: &str, contents: &str, params: &[&str], expRet: i32, expOut: &str)
{
	let (res, out) = RunCnf(name, contents.as_bytes(), params);
	assert_eq!(res, Ok(expRet), "{:?} {:?}:\n{}", contents, params, out);
	assert!(out.contains(expOut), "{:?} {:?}: {:?} is expected in\n{}", contents, params, expOut, out);
}

// The tool must fail with the given uncaught error (a logic_error thrown out of main in the C++ version)
fn ExpectErr(name: &str, contents: &str, params: &[&str], expErr: &str)
{
	let (res, out) = RunCnf(name, contents.as_bytes(), params);
	assert_eq!(res, Err(expErr.to_string()), "{:?} {:?}:\n{}", contents, params, out);
}

// A header and one clause, which create the solver instance
const P: &str = "p cnf 2 1\n1 2 0\n";

#[test]
fn usage()
{
	for args in [&[][..], &["-help"][..], &["--help"][..], &["-h"][..], &["-h", "ignored"][..]]
	{
		let (res, out) = RunTool(args);
		assert_eq!(res, Ok(0));
		assert!(out.contains("c Usage:"));
		assert!(out.contains("\tc s <Lit1 <Lit2> ... <Litn>: solve under the assumptions {<Lit1 <Lit2> ... <Litn>}\n"));
		for param in ["solver_mode", "bin_drat_file", "text_drat_file", "drat_sort_every_clause", "print_model", "print_ucore", "verify_model", "verify_ucore", "ignore_file_params",
			"allsat_models_number", "allsat_blocking_variables", "allsat_blocking_variables_file_alg"]
		{
			assert!(out.contains(&format!("/topor_tool/{}", param)), "{}", param);
		}
		// The parameters of the solver
		assert!(out.contains("/mode/value"));
		assert!(!out.contains("c Intel(R) SAT Solver started"));
	}
}

#[test]
fn command_line_errors()
{
	// An even number of arguments
	let (res, out) = RunTool(&["file.cnf", "/topor_tool/print_model"]);
	assert_eq!(res, Ok(BadRetVal));
	assert!(out.starts_with("c Intel(R) SAT Solver started\n"));
	assert!(out.contains("c topor_tool ERROR: the number of arguments (excluding the executable name) must be odd. Run without parameters for more information.\n"));

	// The input file doesn't exist
	let f = TmpFile("no_such_file.cnf");
	let (res, out) = RunTool(&[&f]);
	assert_eq!(res, Ok(BadRetVal));
	assert!(out.contains(&format!("c topor_tool ERROR: the input file {} doesn't exist\n", f)));

	// The input is a directory: it can be opened, but there is nothing to read
	let d = TmpFile("a_directory.cnf");
	std::fs::create_dir_all(&d).unwrap();
	let (res, out) = RunTool(&[&d]);
	assert_eq!(res, Ok(BadRetVal));
	assert_eq!(out, "c Intel(R) SAT Solver started\n");
	let _ = std::fs::remove_dir(&d);
}

#[test]
fn plain_dimacs()
{
	let sat = "c a comment\np cnf 3 2\n1 2 0\n-1 3 0\n";
	for solverMode in ["0", "1", "2"]
	{
		let (res, out) = RunCnf("plain_sat.cnf", sat.as_bytes(), &["/topor_tool/solver_mode", solverMode, "/topor_tool/verify_model", "1", "/verbosity/level", "0"]);
		assert_eq!(res, Ok(10), "{}", out);
		assert!(out.contains(&format!("c /topor_tool/solver_mode {}\n", solverMode)));
		assert!(out.contains("c topor_tool: suggested #variables : 3; suggested #clauses : 2\n"));
		assert_eq!(CountLines(&out, "s SATISFIABLE"), 1);
		assert_eq!(out.lines().filter(|l| l.starts_with("v ") && l.ends_with(" 0") && l.split(' ').count() == 5).count(), 1, "{}", out);
		assert!(out.contains("c topor_tool: before verifying that the model satisfies the clauses\n"));
		assert!(out.contains("c topor_tool: clauses verified!\n"));

		let (res, out) = RunCnf("plain_unsat.cnf", b"p cnf 1 2\n1 0\n-1 0\n", &["/topor_tool/solver_mode", solverMode, "/topor_tool/print_ucore", "1", "/topor_tool/verify_ucore", "1"]);
		assert_eq!(res, Ok(20), "{}", out);
		// The core is empty; verifying it solves once again
		assert_eq!(CountLines(&out, "s UNSATISFIABLE"), 2, "{}", out);
		assert!(out.contains("v 0\n"));
	}

	// No header; the model isn't printed
	let (res, out) = RunCnf("plain_no_header.cnf", b"1 2 0\n-1 0\n", &["/topor_tool/print_model", "0"]);
	assert_eq!(res, Ok(10));
	assert!(out.contains("c /topor_tool/print_model 0\n"));
	assert!(out.ends_with("s SATISFIABLE\n"), "{}", out);
	assert!(!out.contains("\nv"));

	// The model is printed by default; the unsatisfiable core isn't
	ExpectRet("plain_model.cnf", "1 2 0\n-1 0\n-2 3 0\n", &[], 10, "s SATISFIABLE\nv -1 2 3 0\n");
	let (res, out) = RunCnf("plain_no_ucore.cnf", b"1 0\n-1 0\n", &[]);
	assert_eq!(res, Ok(20));
	assert!(out.ends_with("s UNSATISFIABLE\n"), "{}", out);

	// The header is only a recommendation; a bad number of variables is ignored
	ExpectRet("plain_vars_0.cnf", "p cnf 0 1\n1 0\n", &[], 10, "c topor_tool warning: the suggested #variables 0 is greater than the maximal number or is <=0, thus it will be ignored\n");
	ExpectRet("plain_vars_neg.cnf", "p cnf -5 1\n1 0\n", &[], 10, "c topor_tool warning: the suggested #variables -5 is greater than the maximal number or is <=0, thus it will be ignored\n");
	ExpectRet("plain_vars_big.cnf", "p cnf 99999999999 1\n1 0\n", &[], 10, "c topor_tool warning: the suggested #variables 99999999999 is greater than the maximal number or is <=0, thus it will be ignored\n");

	// Leading white-spaces, a line of white-spaces only at the end of the file, and a 0-character, which ends the line
	ExpectRet("plain_spaces.cnf", "  p cnf 2 2   \n  1   -2 0\n   c comment\n2 0\0 garbage\n   ", &[], 10, "s SATISFIABLE\nv 1 2 0\n");
	// An empty input: there is no solver to query
	let (res, out) = RunCnf("plain_empty.cnf", b"", &[]);
	assert_eq!((res, out.as_str()), (Ok(BadRetVal), "c Intel(R) SAT Solver started\n"));
	let (res, out) = RunCnf("plain_comments.cnf", b"c nothing\nc but comments\n", &[]);
	assert_eq!((res, out.as_str()), (Ok(BadRetVal), "c Intel(R) SAT Solver started\n"));
}

#[test]
fn incremental_commands()
{
	let cnf = concat!(
		"p cnf 4 3\n",
		"1 2 0\n",
		"-1 2 0\n",
		"-2 3 4 0\n",
		"r /verbosity/level 0\n",
		"n 0\n",
		"n 3\n",
		"lb 1 2.5\n",
		"lf -1 0\n",
		"lf 3 1\n",
		"lc 1\n",
		"ll 4\n",
		"ot 1000 1\n",
		"oc 100000\n",
		"s -2 0\n",
		"ot 1000.5 0\n",
		"s 2 -3 -4 1 0\n",
		"s 1 0\n",
		"b 0\n",
		"s 0\n",
		"-3 0\n",
		"s -4 0\n",
		"s 0\n");
	for solverMode in ["0", "1", "2"]
	{
		let (res, out) = RunCnf("incremental.cnf", cnf.as_bytes(), &["/topor_tool/solver_mode", solverMode, "/topor_tool/print_ucore", "1", "/topor_tool/verify_model", "1"]);
		assert_eq!(res, Ok(10), "{}", out);
		assert!(!out.contains("ERROR"), "{}", out);
		assert!(out.contains("c converted configuration number 0 to parameters \n"), "{}", out);
		assert!(out.contains("c converted configuration number 3 to parameters /mode/value 3\n"), "{}", out);
		let verdicts: Vec<&str> = out.lines().filter(|l| l.starts_with("s ")).collect();
		assert_eq!(verdicts, ["s UNSATISFIABLE", "s UNSATISFIABLE", "s SATISFIABLE", "s SATISFIABLE", "s UNSATISFIABLE", "s SATISFIABLE"], "{}", out);
		// The unsatisfiable cores
		assert!(out.contains("s UNSATISFIABLE\nv -2 0\n"), "{}", out);
		assert!(out.contains("s UNSATISFIABLE\nv 2 -3 -4 0\n"), "{}", out);
		assert!(out.contains("s UNSATISFIABLE\nv -4 0\n"), "{}", out);
		assert!(out.contains("c topor_tool: before verifying that the model satisfies the assumptions and the clauses\nc topor_tool: assumptions verified!\nc topor_tool: clauses verified!\n"), "{}", out);
		assert!(out.ends_with("s SATISFIABLE\nv 1 2 -3 4 0\nc topor_tool: before verifying that the model satisfies the assumptions and the clauses\nc topor_tool: assumptions verified!\nc topor_tool: clauses verified!\n"), "{}", out);

		// Verifying the cores
		let (res, out) = RunCnf("incremental_vu.cnf", cnf.as_bytes(), &["/topor_tool/solver_mode", solverMode, "/topor_tool/verify_ucore", "1"]);
		assert_eq!(res, Ok(10), "{}", out);
		assert!(out.contains("Assumption #0 -- -2 : 1\ns UNSATISFIABLE\n"), "{}", out);
		assert!(out.contains("Assumption #0 -- 2 : 1\nAssumption #1 -- -3 : 1\nAssumption #2 -- -4 : 1\nAssumption #3 -- 1 : 0\ns UNSATISFIABLE\n"), "{}", out);
		assert_eq!(CountLines(&out, "s UNSATISFIABLE"), 6, "{}", out);
		assert!(!out.contains("BUG"), "{}", out);
	}

	// The thresholds hold for the next query only
	let (res, out) = RunCnf("incremental_thr.cnf", b"1 2 0\not 0 0\ns 0\not 0 1\ns 0\ns 0\n", &[]);
	assert!(res == Ok(10) || res == Ok(BadRetVal), "{}", out);
	// The parameters in the file can be ignored
	ExpectRet("incremental_r_err.cnf", &(P.to_string() + "r /no/such/parameter 1\ns 0\ns 1 0\n"), &[], BadRetVal, "s PARAM_ERROR\nc ");
	ExpectRet("incremental_r_err_final.cnf", &(P.to_string() + "r /no/such/parameter 1\n"), &[], BadRetVal, "s PARAM_ERROR\nc ");
	let (res, out) = RunCnf("incremental_r_ignored.cnf", (P.to_string() + "r /no/such/parameter 1\nr\ns 0\n").as_bytes(), &["/topor_tool/ignore_file_params", "1"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(out.contains("c /topor_tool/ignore_file_params 1\n"));
}

#[test]
fn incremental_command_errors()
{
	let E = |cmd: &str, expErr: &str| ExpectErr("incremental_err.cnf", &(P.to_string() + cmd), &[], &("c topor_tool ERROR: ".to_string() + expErr + " at line number 3"));
	// r
	E("r\n", "expected <paramName> never ended");
	E("r /verbosity/level\n", "expected <paramName> never ended");
	E("r /verbosity/level x\n", "couldn't convert the parameter value to double");
	// ot and oc
	E("ox 1\n", "The 2nd character must be either t or c");
	E("o", "The 2nd character must be either t or c");
	E("otx\n", "The 3nd character must be a space");
	E("oc", "The 3nd character must be a space");
	E("ot 5\n", "expected <TimeOut> <IsCpuTimeOut>");
	E("ot x 1\n", "couldn't convert <TimeOut> to double");
	E("ot 5 x\n", "couldn't convert <IsCpuTimeOut> to int");
	E("ot 5 2\n", "couldn't convert <IsCpuTimeOut> to 0 or 1");
	E("ot 5 -1\n", "couldn't convert <IsCpuTimeOut> to 0 or 1");
	E("oc x\n", "couldn't convert <ConflictThreshold> to uint64_t");
	// lb, lf, lc and ll
	E("lx 1\n", "The 2nd character must be either b or f or c");
	E("lbx\n", "The 3nd character must be a space");
	E("lb x\n", "the first character is expected to be a digit");
	E("lc -x\n", "the first character is expected to be a digit");
	E("ll ", "no number after skipping white-spaces");
	E("lf 1 x\n", "couldn't convert <FixPolarityLit> to int");
	E("lf 1 2\n", "couldn't convert <FixPolarityLit> to 0 or 1");
	E("lf 1 -1\n", "couldn't convert <FixPolarityLit> to 0 or 1");
	E("lf 1\n", "couldn't convert <FixPolarityLit> to int");
	E("lb 1 x\n", "couldn't convert the <Mult> value to double");
	E("lb 1\n", "couldn't convert the <Mult> value to double");
	// b
	E("bx\n", "The 2nd character must be a space");
	E("b x\n", "the first character is expected to be a digit");
	// n
	E("nx\n", "The 2nd character must be a space");
	E("n x\n", "the first character is expected to be a digit");
	E("n -1\n", "The configuration number -1 must be a uint_16 integer");
	E("n 65536\n", "The configuration number 65536 must be a uint_16 integer");

	// std::out_of_range of substr in the C++ version: nothing at all after the literal
	ExpectErr("incremental_err_lf.cnf", &(P.to_string() + "lf 1"), &[], "basic_string::substr");
	ExpectErr("incremental_err_lb.cnf", &(P.to_string() + "lb 1"), &[], "basic_string::substr");

	// The commands, which don't require the solver, are legal before the header too
	ExpectErr("incremental_err_first.cnf", "ot 1 1\noc 5\nox\n", &[], "c topor_tool ERROR: The 2nd character must be either t or c at line number 3");
	// The commands, which require the solver, are not (the C++ version dereferences a null pointer)
	for cmd in ["b 0", "n 1", "lc 1", "r /mode/value 1"]
	{
		ExpectErr("incremental_err_before_header.cnf", &(cmd.to_string() + "\np cnf 1 1\n1 0\n"), &[], "c topor_tool ERROR: a solver command appears before the p-line and the first clause at line number 1");
	}
}

#[test]
fn clause_and_header_errors()
{
	let lineErr = |lineNum: u64| format!("c topor_tool ERROR: couldn't translate the following line or parts of it into a vector of literals at line number {}\n", lineNum);
	// Clauses
	ExpectRet("cls_err_0.cnf", "1 2 0\n1 x 0\n", &[], BadRetVal, &lineErr(2));
	ExpectRet("cls_err_1.cnf", "1 2\n", &[], BadRetVal, &lineErr(1));
	ExpectRet("cls_err_2.cnf", "1 2", &[], BadRetVal, &lineErr(1));
	ExpectRet("cls_err_3.cnf", "1 - 0\n", &[], BadRetVal, &lineErr(1));
	// An empty line isn't legal
	ExpectRet("cls_err_4.cnf", "1 0\n\n", &[], BadRetVal, &lineErr(2));
	ExpectRet("cls_err_5.cnf", "1 99999999999 0\n", &[], BadRetVal, "c topor_tool ERROR: the literal 99999999999 is too big or too small\n");
	ExpectRet("cls_err_6.cnf", "1 -99999999999 0\n", &[], BadRetVal, "c topor_tool ERROR: the literal -99999999999 is too big or too small\n");
	// Assumptions
	ExpectRet("cls_err_7.cnf", "1 0\ns x\n", &[], BadRetVal, &lineErr(2));
	ExpectRet("cls_err_8.cnf", "s 1 2147483648 0\n", &[], BadRetVal, "c topor_tool ERROR: the literal 2147483648 is too big or too small\n");

	// The header
	ExpectRet("p_err_0.cnf", &(P.to_string() + "p cnf 2 1\n"), &[], BadRetVal, "c topor_tool ERROR: second line starting with p at line number 3\n");
	ExpectRet("p_err_1.cnf", "1 0\np cnf 2 1\n", &[], BadRetVal, "c topor_tool ERROR: second line starting with p at line number 2\n");
	for header in ["pcnf 2 1\n", "p dnf 2 1\n", "p  cnf 2 1\n", "p cnf2 1\n", "p cmf 2 1\n", "p cnt 2 1\n", "p"]
	{
		ExpectRet("p_err_2.cnf", header, &[], BadRetVal, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>' at line number 1\n");
	}
	ExpectRet("p_err_3.cnf", "p cnf x 1\n", &[], BadRetVal,
		"c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': c topor_tool ERROR: the first character is expected to be a digit at line number 1 at line number 1\n");
	ExpectRet("p_err_4.cnf", "p cnf 3\n", &[], BadRetVal,
		"c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': c topor_tool ERROR: the first character is expected to be a digit at line number 1 at line number 1\n");
	ExpectRet("p_err_5.cnf", "p cnf 3 ", &[], BadRetVal,
		"c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': c topor_tool ERROR: no number after skipping white-spaces at line number 1 at line number 1\n");
	ExpectRet("p_err_6.cnf", "p cnf 2 1 x\n", &[], BadRetVal, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': new-line wasn't found where expected at line number 1\n");
	ExpectRet("p_err_7.cnf", "p cnf 2 1", &[], BadRetVal, "c topor_tool ERROR: couldn't parse the p-line as 'p cnf <VARS> <CLSS>': new-line wasn't found where expected at line number 1\n");
}

#[test]
fn parameter_errors()
{
	// The parameters are parsed when the solver is created: either by the header or by the first clause or query
	for cnf in ["p cnf 1 1\n1 0\n", "1 0\n", "s 1 0\n"]
	{
		let E = |params: &[&str], expOut: &str| ExpectRet("param_err.cnf", cnf, params, BadRetVal, expOut);
		for param in ["drat_sort_every_clause", "print_model", "print_ucore", "verify_model", "verify_ucore", "ignore_file_params"]
		{
			let name = format!("/topor_tool/{}", param);
			E(&[&name, "x"], &format!("c {} x\nc ERROR: couldn't convert x to an integer\n", name));
			E(&[&name, "2"], &format!("c {} 2\nc ERROR: 2 must be 0 or 1\n", name));
			E(&[&name, "-1"], &format!("c {} -1\nc ERROR: -1 must be 0 or 1\n", name));
		}
		E(&["/topor_tool/solver_mode", "x"], "c /topor_tool/solver_mode x\nc ERROR: couldn't convert x to an integer\n");
		E(&["/topor_tool/solver_mode", "3"], "c /topor_tool/solver_mode 3\nc ERROR: 3 must be 0 or 1 or 2\n");
		E(&["/verbosity/level", "0", "/topor_tool/solver_mode", "1"], "c /topor_tool/solver_mode 1\nc topor_tool ERROR: /topor_tool/solver_mode should be provided before any other parameters\n");
		E(&["/topor_tool/allsat_models_number", "x"], "c /topor_tool/allsat_models_number x\nc ERROR: couldn't convert x to an unsigned long\n");
		E(&["/topor_tool/allsat_blocking_variables", "1,x"], "c /topor_tool/allsat_blocking_variables 1,x\nc ERROR: couldn't convert 1,x to a variable list\n");
		E(&["/topor_tool/allsat_blocking_variables_file_alg", ""], "c /topor_tool/allsat_blocking_variables_file_alg \nc ERROR: couldn't convert  to an unsigned long\n");
		E(&["/topor_tool/no_such_parameter", "1"], "c ERROR: unrecognized /topor_tool/ parameter: /topor_tool/no_such_parameter\n");
		E(&["/topor_tool/", "1"], "c ERROR: unrecognized /topor_tool/ parameter: /topor_tool/\n");
		E(&["/verbosity/level", "x"], "c topor_tool ERROR: could not convert x to double\n");
		E(&["/x", "1"], "c ERROR in Topor parameter: ");
		E(&["/no/such/parameter", "1"], "c ERROR in Topor parameter: ");
	}

	// The legal values
	let (res, out) = RunCnf("param_ok.cnf", b"p cnf 1 1\n1 0\n", &["/topor_tool/solver_mode", "0", "/topor_tool/drat_sort_every_clause", "0", "/topor_tool/print_model", "1", "/topor_tool/print_ucore", "0",
		"/topor_tool/verify_model", "0", "/topor_tool/verify_ucore", "0", "/topor_tool/ignore_file_params", "0", "/topor_tool/allsat_models_number", "1", "/topor_tool/allsat_blocking_variables", "1",
		"/topor_tool/allsat_blocking_variables_file_alg", "0", "/verbosity/level", "0"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(out.ends_with("c /topor_tool/allsat_blocking_variables_file_alg 0\ns SATISFIABLE\nv 1 0\n"), "{}", out);
}

#[test]
fn drat_files()
{
	let unsat = "p cnf 2 4\n1 2 0\n-1 2 0\n1 -2 0\n-1 -2 0\n";
	let text = TmpFile("drat_text.drat");
	let bin = TmpFile("drat_bin.drat");
	// Only the last DRAT file parameter is applied
	let (res, out) = RunCnf("drat_0.cnf", unsat.as_bytes(), &["/topor_tool/bin_drat_file", &bin, "/topor_tool/text_drat_file", &text, "/topor_tool/drat_sort_every_clause", "1"]);
	assert_eq!(res, Ok(20), "{}", out);
	assert!(out.contains(&format!("c /topor_tool/bin_drat_file {}\nc /topor_tool/text_drat_file {}\nc /topor_tool/drat_sort_every_clause 1\n", bin, text)));
	assert!(!std::path::Path::new(&bin).exists());
	let proof = std::fs::read_to_string(&text).unwrap();
	// The empty clause is derived
	assert!(proof.lines().any(|l| l == "0"), "{}", proof);
	assert!(proof.lines().all(|l| l.starts_with("c ") || l.bytes().all(|c| c == b'-' || c == b'd' || c == b' ' || c.is_ascii_digit())), "{}", proof);
	let _ = std::fs::remove_file(&text);

	let (res, out) = RunCnf("drat_1.cnf", unsat.as_bytes(), &["/topor_tool/text_drat_file", &text, "/topor_tool/bin_drat_file", &bin]);
	assert_eq!(res, Ok(20), "{}", out);
	assert!(!std::path::Path::new(&text).exists());
	let proof = std::fs::read(&bin).unwrap();
	assert!(!proof.is_empty() && proof[0] == b'a' && *proof.last().unwrap() == 0, "{:?}", proof);
	let _ = std::fs::remove_file(&bin);

	// The DRAT file cannot be opened: the solver discovers the problem when it writes to it
	let bad = TmpFile("no_such_dir") + "/proof.drat";
	let (res, out) = RunCnf("drat_2.cnf", unsat.as_bytes(), &["/topor_tool/bin_drat_file", &bad]);
	assert_eq!(res, Ok(BadRetVal), "{}", out);
	assert!(out.contains("s DRAT_FILE_PROBLEM\nc "), "{}", out);

	let mut failed = CFailedOfstream;
	assert!(failed.write(b"1 0\n").is_err());
	assert!(failed.flush().is_err());
}

#[test]
fn allsat()
{
	let models = |out: &str| -> Vec<String> { out.lines().filter(|l| l.starts_with("v ")).map(|l| l.to_string()).collect() };
	let cnf = "p cnf 3 1\n1 2 3 0\n";

	// All the models over the blocking variables 1 and 2: {1, 2}, {1, -2}, {-1, 2} and {-1, -2} (where 3 must hold)
	let (res, out) = RunCnf("allsat_0.cnf", cnf.as_bytes(), &["/topor_tool/allsat_models_number", "10", "/topor_tool/allsat_blocking_variables", "1,2", "/topor_tool/verify_model", "1"]);
	assert_eq!(res, Ok(20), "{}", out);
	assert!(out.contains("c /topor_tool/allsat_models_number 10\nc /topor_tool/allsat_blocking_variables 1,2\n"));
	let mut m = models(&out);
	m.sort();
	assert_eq!(m, ["v -1 -2 0", "v -1 2 0", "v 1 -2 0", "v 1 2 0"], "{}", out);
	assert_eq!(CountLines(&out, "s SATISFIABLE"), 4);
	assert_eq!(CountLines(&out, "s UNSATISFIABLE"), 1);
	assert_eq!(CountLines(&out, "c topor_tool: clauses verified!"), 4);
	assert!(out.contains("c topor_tool: before adding a blocking clause and calling the solver for time 2 out of 10\n"));
	assert!(out.contains("c topor_tool: before adding a blocking clause and calling the solver for time 5 out of 10\n"));
	assert!(!out.contains("for time 6 out of 10"));

	// The number of models is bounded
	let (res, out) = RunCnf("allsat_1.cnf", cnf.as_bytes(), &["/topor_tool/allsat_models_number", "3", "/topor_tool/allsat_blocking_variables", "2,1"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert_eq!(models(&out).len(), 3, "{}", out);
	assert_eq!(CountLines(&out, "s SATISFIABLE"), 3);

	// One model only: a plain query
	let (res, out) = RunCnf("allsat_2.cnf", cnf.as_bytes(), &["/topor_tool/allsat_models_number", "1", "/topor_tool/allsat_blocking_variables", "1,2"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert_eq!(models(&out).len(), 1, "{}", out);
	assert_eq!(models(&out)[0].split(' ').count(), 5, "{}", out);

	// An unsatisfiable instance
	let (res, out) = RunCnf("allsat_3.cnf", b"1 0\n-1 0\n", &["/topor_tool/allsat_models_number", "5", "/topor_tool/allsat_blocking_variables", "1"]);
	assert_eq!(res, Ok(20), "{}", out);
	assert_eq!(CountLines(&out, "s UNSATISFIABLE"), 1);

	// The blocking variables are read from the first comment of the file
	for alg in ["1", "2", "3", "7"]
	{
		for cnf in ["p cnf 4 2\nc 1,2\nc 3,4 isn't read anymore\n1 2 3 0\n-4 0\n", "p cnf 4 2\n1 2 3 0\n-4 0\nc 1,2\n"]
		{
			let (res, out) = RunCnf("allsat_4.cnf", cnf.as_bytes(), &["/topor_tool/allsat_models_number", "10", "/topor_tool/allsat_blocking_variables_file_alg", alg, "/topor_tool/verify_model", "1"]);
			assert_eq!(res, Ok(20), "alg {}: {}", alg, out);
			assert!(out.contains(&format!("c /topor_tool/allsat_blocking_variables_file_alg {}\n", alg)));
			let mut m = models(&out);
			m.sort();
			assert_eq!(m, ["v -1 -2 0", "v -1 2 0", "v 1 -2 0", "v 1 2 0"], "alg {}: {}", alg, out);
		}
	}

	// The blocking variables aren't read from the file: a plain query
	let (res, out) = RunCnf("allsat_5.cnf", b"p cnf 2 1\nc 1,2\n1 2 0\n", &["/topor_tool/allsat_models_number", "10", "/topor_tool/allsat_blocking_variables_file_alg", "0"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert_eq!(models(&out).len(), 1, "{}", out);
	// The blocking variables of the command line take precedence
	let (res, out) = RunCnf("allsat_6.cnf", b"p cnf 2 1\nc 1,2\n1 2 0\n", &["/topor_tool/allsat_models_number", "10", "/topor_tool/allsat_blocking_variables", "2"]);
	assert_eq!(res, Ok(20), "{}", out);
	assert_eq!(models(&out), ["v 2 0", "v -2 0"], "{}", out);

	// The first comment doesn't contain the blocking variables
	let params = ["/topor_tool/allsat_models_number", "2"];
	ExpectErr("allsat_7.cnf", "p cnf 2 1\nc foo\n", &params,
		"c topor_tool ERROR: expected the first comment to contain blocking variables at line number 2. Error message: c ERROR: couldn't convert foo\n to a variable list\n");
	ExpectErr("allsat_8.cnf", "p cnf 2 1\nc", &params,
		"c topor_tool ERROR: expected the first comment to contain blocking variables at line number 2. Error message: c ERROR: couldn't convert  to a variable list\n");
}

// Rust-only extension (topi_ext_hints.rs): the hints files and the lh and ls commands
#[test]
fn hints()
{
	// The variables 1..=4 are free (positive by default), and 5 is implied by the negation of any of them
	let cnf = "p cnf 5 4\n1 5 0\n2 5 0\n3 5 0\n4 5 0\n";
	let on = ["/phase/hints/on", "1", "/decision/score_hints/mult", "2"];
	let hintsFile = TmpFile("hints_in.txt");
	let writtenFile = TmpFile("hints_out.txt");
	std::fs::write(&hintsFile, "c hints\nv -1 -2 0\nb 3 0.5\n").unwrap();

	// The hints are ignored, unless their parameters are on
	let (res, out) = RunCnf("hints_0.cnf", cnf.as_bytes(), &["/topor_tool/hints_file", &hintsFile]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(out.contains(&format!("c /topor_tool/hints_file {}\n", hintsFile)), "{}", out);
	assert!(out.contains(&format!("c topor_tool: accepted 0 phase hints and 0 score hints of {} (", hintsFile)), "{}", out);
	assert!(out.ends_with("s SATISFIABLE\nv 1 2 3 4 5 0\n"), "{}", out);

	// The hinted polarities are followed, and the hints for the next run are written
	for solverMode in ["0", "1", "2"]
	{
		let mut params = vec!["/topor_tool/solver_mode", solverMode, "/topor_tool/hints_file", &hintsFile, "/topor_tool/write_hints_file", &writtenFile, "/topor_tool/verify_model", "1"];
		params.extend_from_slice(&on);
		let (res, out) = RunCnf("hints_1.cnf", cnf.as_bytes(), &params);
		assert_eq!(res, Ok(10), "{}", out);
		assert!(out.contains(&format!("c topor_tool: accepted 2 phase hints and 1 score hints of {} (", hintsFile)), "{}", out);
		assert!(out.contains("s SATISFIABLE\nv -1 -2 3 4 5 0\n"), "{}", out);
		let written = std::fs::read_to_string(&writtenFile).unwrap();
		assert!(written.contains("\nv -1 -2 3 4 5 0\n") && written.contains("\nb 3 "), "{}", written);
		// The written file is a legal hints file
		let mut params = vec!["/topor_tool/hints_file", &writtenFile[..]];
		params.extend_from_slice(&on);
		let (res, out) = RunCnf("hints_2.cnf", cnf.as_bytes(), &params);
		assert_eq!(res, Ok(10), "{}", out);
		assert!(out.ends_with("s SATISFIABLE\nv -1 -2 3 4 5 0\n"), "{}", out);
	}

	// The hints are written after every query, also in the AllSAT mode
	let (res, out) = RunCnf("hints_3.cnf", (cnf.to_string() + "s -3 0\ns 0\n").as_bytes(), &["/topor_tool/write_hints_file", &writtenFile]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(std::fs::read_to_string(&writtenFile).unwrap().contains("\nv 1 2 -3 4 5 0\n"));
	let (res, out) = RunCnf("hints_4.cnf", cnf.as_bytes(), &["/topor_tool/write_hints_file", &writtenFile, "/topor_tool/allsat_models_number", "2", "/topor_tool/allsat_blocking_variables", "1"]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(std::fs::read_to_string(&writtenFile).unwrap().contains("\nv -1 "));

	// The commands
	let (res, out) = RunCnf("hints_5.cnf", (cnf.to_string() + "r /phase/hints/on 1\nr /decision/score_hints/mult 1\nlh -4\nls 2 0.75\ns 0\nlh -3\ns 4 0\n").as_bytes(), &[]);
	assert_eq!(res, Ok(10), "{}", out);
	assert!(out.contains("s SATISFIABLE\nv 1 2 3 -4 5 0\n") && out.ends_with("s SATISFIABLE\nv 1 2 -3 4 5 0\n"), "{}", out);
	ExpectErr("hints_6.cnf", &(P.to_string() + "ls 1 x\n"), &[], "c topor_tool ERROR: couldn't convert the <Score> value to double at line number 3");
	ExpectErr("hints_7.cnf", &(P.to_string() + "ls 1"), &[], "c topor_tool ERROR: couldn't convert the <Score> value to double at line number 3");

	// The files, which cannot be read, parsed or written
	let missing = TmpFile("no_such_hints.txt");
	ExpectRet("hints_8.cnf", cnf, &["/topor_tool/hints_file", &missing], BadRetVal, &format!("c topor_tool ERROR: couldn't read the hints file {}: ", missing));
	std::fs::write(&hintsFile, "v 1 2\nb 1\n").unwrap();
	ExpectRet("hints_9.cnf", cnf, &["/topor_tool/hints_file", &hintsFile], BadRetVal, &format!("c topor_tool ERROR: couldn't read the hints file {}: hints: cannot parse line 2: b 1\n", hintsFile));
	let unwritable = TmpFile("no_such_dir") + "/hints.txt";
	ExpectRet("hints_10.cnf", cnf, &["/topor_tool/write_hints_file", &unwritable], 10, &format!("c topor_tool ERROR: couldn't write the hints file {}: ", unwritable));

	// The usage
	let (_, out) = RunTool(&[]);
	assert!(out.contains("/topor_tool/hints_file") && out.contains("/topor_tool/write_hints_file") && out.contains("\tc lh <PhaseHintLit>") && out.contains("\tc ls <ScoreHintVar> <Score>"));
	assert!(out.contains("/phase/hints/on") && out.contains("/decision/score_hints/mult"));
	let _ = std::fs::remove_file(&hintsFile);
	let _ = std::fs::remove_file(&writtenFile);
}

/*
* The free functions and the internals of the tool
*/

#[test]
fn on_finishing_solving()
{
	// A fresh solver: there are no variables and no status explanation
	let mut topor = CTopor::<u32, false>::new(0);
	for (ret, expRet, expOut) in [
		(TToporReturnVal::RET_SAT, 10, "s SATISFIABLE\nv 0\n"),
		(TToporReturnVal::RET_UNSAT, 20, "s UNSATISFIABLE\nv 0\n"),
		(TToporReturnVal::RET_TIMEOUT_LOCAL, BadRetVal, "s TIMEOUT_LOCAL\n"),
		(TToporReturnVal::RET_CONFLICT_OUT, 30, "s CONFLICT_OUT\n"),
		(TToporReturnVal::RET_MEM_OUT, BadRetVal, "s MEMORY_OUT\n"),
		(TToporReturnVal::RET_USER_INTERRUPT, BadRetVal, "s USER_INTERRUPT\n"),
		(TToporReturnVal::RET_INDEX_TOO_NARROW, BadRetVal, "s INDEX_TOO_NARROW\n"),
		(TToporReturnVal::RET_PARAM_ERROR, BadRetVal, "s PARAM_ERROR\n"),
		(TToporReturnVal::RET_TIMEOUT_GLOBAL, BadRetVal, "s TIMEOUT_GLOBAL\n"),
		(TToporReturnVal::RET_DRAT_FILE_PROBLEM, BadRetVal, "s DRAT_FILE_PROBLEM\n"),
		(TToporReturnVal::RET_EXOTIC_ERROR, BadRetVal, "s EXOTIC_ERROR\n"),
		(TToporReturnVal::RET_ASSUMPTION_REQUIRED_ERROR, BadRetVal, "s UNEXPECTED_ERROR\n")]
	{
		let mut out: Vec<u8> = vec![];
		assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, true, &[], None), expRet);
		assert_eq!(String::from_utf8_lossy(&out), expOut);
	}

	// A real query: the model, the given variables only, the unsatisfiable core and the status explanation
	let mut topor = CTopor::<u64, true>::new(0);
	topor.ToporAddClause(&[1, 2]);
	topor.ToporAddClause(&[-1]);
	topor.ToporAddClause(&[-2, 3]);
	let ret = topor.ToporSolve(&[], (f64::MAX, true), u64::MAX);
	assert_eq!(ret, TToporReturnVal::RET_SAT);
	assert_eq!(topor.ToporGetSolveInvs(), 1);
	assert_eq!(topor.ToporGetMaxUserVar(), 3);
	assert_eq!(topor.ToporGetLitValue(2), TToporLitVal::VAL_SATISFIED);
	assert_eq!(topor.ToporGetLitDecLevel(2), 0);
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, false, &[], None), 10);
	assert_eq!(String::from_utf8_lossy(&out), "s SATISFIABLE\nv -1 2 3 0\n");
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, false, &[], Some(&[3, 1])), 10);
	assert_eq!(String::from_utf8_lossy(&out), "s SATISFIABLE\nv 3 -1 0\n");
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, false, true, &[], None), 10);
	assert_eq!(String::from_utf8_lossy(&out), "s SATISFIABLE\n");

	let assumps = [3, -2, 1];
	let ret = topor.ToporSolve(&assumps, (f64::MAX, true), u64::MAX);
	assert_eq!(ret, TToporReturnVal::RET_UNSAT);
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, true, &assumps, None), 20);
	let out = String::from_utf8_lossy(&out).into_owned();
	assert!(out == "s UNSATISFIABLE\nv -2 0\n" || out == "s UNSATISFIABLE\nv 1 0\n", "{}", out);
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, false, &assumps, None), 20);
	assert_eq!(String::from_utf8_lossy(&out), "s UNSATISFIABLE\n");

	topor.ToporSetParam("/no/such/parameter", 1.);
	assert!(topor.ToporIsError());
	let expl = topor.ToporGetStatusExplanation();
	assert!(!expl.is_empty());
	let ret = topor.ToporSolve(&[], (f64::MAX, true), u64::MAX);
	let mut out: Vec<u8> = vec![];
	assert_eq!(OnFinishingSolving(&mut out, &mut topor, ret, true, true, &[], None), BadRetVal);
	assert_eq!(String::from_utf8_lossy(&out), format!("s PARAM_ERROR\nc {}\n", expl));
}

#[test]
fn string_stream_and_var_list()
{
	let mut ss = CStringStream::new(b"12,3");
	assert!(ss.good());
	assert_eq!(ss.getline(b','), b"12");
	assert!(ss.good());
	// Overwriting the beginning of the buffer
	ss.put(b"ab");
	assert_eq!(ss.m_Buf, b"ab,3");
	assert_eq!(ss.getline(b','), b"3");
	// The end of the stream has been reached: nothing can be read or written anymore
	assert!(!ss.good());
	assert_eq!(ss.getline(b','), b"");
	ss.put(b"xyz");
	assert_eq!(ss.m_Buf, b"ab,3");
	// Writing beyond the end of the buffer
	let mut ss = CStringStream::new(b"1,");
	ss.put(b"987");
	assert_eq!(ss.m_Buf, b"987");
	assert_eq!(ss.getline(b','), b"987");
	assert!(!ss.good());

	let R = |s: &str| ReadCommaSeparatedVarList(s.as_bytes());
	assert_eq!(R("1,4,15"), Ok(vec![1, 4, 15]));
	assert_eq!(R("7"), Ok(vec![7]));
	assert_eq!(R("7\n"), Ok(vec![7]));
	// A trailing comma is an error: there is nothing to convert after it
	assert_eq!(R("7,8,"), Err("c ERROR: couldn't convert 7,8, to a variable list\n".to_string()));
	assert_eq!(R(""), Err("c ERROR: couldn't convert  to a variable list\n".to_string()));
	assert_eq!(R("1,x,3"), Err("c ERROR: couldn't convert 1,x,3 to a variable list\n".to_string()));
	assert_eq!(R("1,,3"), Err("c ERROR: couldn't convert 1,,3 to a variable list\n".to_string()));
	assert_eq!(R("99999999999999999999"), Err("c ERROR: couldn't convert 99999999999999999999 to a variable list\n".to_string()));
	// A non-positive variable at the end of the list: precisely as in the C++ version, the error message is lost, since the stream isn't good anymore
	assert_eq!(R("1,0"), Ok(vec![1, 0]));
	assert_eq!(R("1,-2"), Ok(vec![1, -2]));
	// A non-positive variable in the middle of the list: precisely as in the C++ version, the error message goes into the stream the variables are read from, so reading the next variable fails
	assert_eq!(R("-2,3"), Err("c ERROR: couldn't convert -2,3 to a variable list\n".to_string()));
	assert_eq!(R("0,5"), Err("c ERROR: couldn't convert 0,5 to a variable list\n".to_string()));
}

#[test]
fn line_parsing()
{
	// The line ends at the first 0-character
	let l = CLine::new(b"ab c\0de".to_vec());
	assert_eq!((l.len, l.currLineI), (4, 0));
	assert_eq!((l.at(0), l.at(3), l.at(4), l.at(100)), (b'a', b'c', 0, 0));
	assert_eq!(l.curr(), b'a');
	assert_eq!(l.substr(0), Some("ab c".to_string()));
	assert_eq!(l.substr(3), Some("c".to_string()));
	assert_eq!(l.substr(4), Some(String::new()));
	assert_eq!(l.substr(5), None);
	assert_eq!(l.substrn(1, 2), Some("b ".to_string()));
	assert_eq!(l.substrn(1, 100), Some("b c".to_string()));
	assert_eq!(l.substrn(4, 1), Some(String::new()));
	assert_eq!(l.substrn(5, 1), None);
	assert_eq!(l.find(b' ', 0), Some(2));
	assert_eq!(l.find(b' ', 2), Some(2));
	assert_eq!(l.find(b' ', 3), None);
	assert_eq!(l.find(b'a', 4), None);
	assert_eq!(l.find(b'd', 0), None);

	let mut l = CLine::new(b"   ".to_vec());
	l.SkipWhitespaces();
	assert_eq!(l.currLineI, 3);
	assert_eq!(l.curr(), 0);
	assert_eq!(l.ParseNumber(5), Err("c topor_tool ERROR: no number after skipping white-spaces at line number 5".to_string()));

	let mut l = CLine::new(b"  12 -345 0 - 7 x\n".to_vec());
	assert_eq!(l.ParseNumber(1), Ok(12));
	assert_eq!(l.ParseNumber(1), Ok(-345));
	assert_eq!(l.ParseNumber(1), Ok(0));
	assert_eq!(l.ParseNumber(8), Err("c topor_tool ERROR: the first character is expected to be a digit at line number 8".to_string()));
	assert_eq!(l.ParseNumber(1), Ok(7));
	assert_eq!(l.ParseNumber(9), Err("c topor_tool ERROR: the first character is expected to be a digit at line number 9".to_string()));
	// A number, which doesn't fit into 64 bits, wraps around
	let mut l = CLine::new(b"18446744073709551617 -18446744073709551617".to_vec());
	assert_eq!(l.ParseNumber(1), Ok(1));
	assert_eq!(l.ParseNumber(1), Ok(-1));

	let B = |s: &str| CLine::new(s.as_bytes().to_vec()).BufferToLits(3);
	assert_eq!(B("1 -2 3 0\n"), (String::new(), vec![1, -2, 3, 0]));
	assert_eq!(B("0"), (String::new(), vec![0]));
	// Whatever follows the 0 is ignored
	assert_eq!(B(" 2147483647   -2147483648 0 5 x"), (String::new(), vec![TLit::MAX, TLit::MIN, 0]));
	assert_eq!(B("1 2147483648 0\n"), ("c topor_tool ERROR: the literal 2147483648 is too big or too small\n".to_string(), vec![]));
	assert_eq!(B("1 -2147483649 0\n"), ("c topor_tool ERROR: the literal -2147483649 is too big or too small\n".to_string(), vec![]));
	let errorString = "c topor_tool ERROR: couldn't translate the following line or parts of it into a vector of literals at line number 3\n";
	assert_eq!(B("1 2\n"), (errorString.to_string(), vec![]));
	assert_eq!(B("1 2"), (errorString.to_string(), vec![]));
	assert_eq!(B(""), (errorString.to_string(), vec![]));
	assert_eq!(B("1 a 0"), (errorString.to_string(), vec![]));
}

#[test]
fn param_readers()
{
	fn E<T>(s: &str) -> Result<T, String>
	{
		Err(s.to_string())
	}
	assert_eq!(CToporTool::ReadBoolParam("0"), Ok(false));
	assert_eq!(CToporTool::ReadBoolParam("1"), Ok(true));
	assert_eq!(CToporTool::ReadBoolParam("5"), E("c ERROR: 5 must be 0 or 1\n"));
	assert_eq!(CToporTool::ReadBoolParam("y"), E("c ERROR: couldn't convert y to an integer\n"));
	// Out of the range of int
	assert_eq!(CToporTool::ReadBoolParam("99999999999"), E("c ERROR: couldn't convert 99999999999 to an integer\n"));

	for (s, v) in [("0", 0), ("1", 1), ("2", 2)]
	{
		assert_eq!(CToporTool::Read0to2Param(s), Ok(v));
	}
	assert_eq!(CToporTool::Read0to2Param("3"), E("c ERROR: 3 must be 0 or 1 or 2\n"));
	assert_eq!(CToporTool::Read0to2Param("two"), E("c ERROR: couldn't convert two to an integer\n"));

	assert_eq!(CToporTool::ReadULongParam("18446744073709551615"), Ok(u64::MAX));
	assert_eq!(CToporTool::ReadULongParam("many"), E("c ERROR: couldn't convert many to an unsigned long\n"));
}

// A solver, which answers as told: for the paths of the tool, which a correct solver never triggers (wrong models and wrong unsatisfiable cores)
struct CMockTopor
{
	// The return value of every Solve invocation (the last one is repeated)
	m_Rets: Vec<TToporReturnVal>,
	// The value of every positive literal after every Solve invocation (the last one is repeated)
	m_Vals: Vec<TToporLitVal>,
	m_Solves: usize,
	m_Clss: usize,
}

impl CMockTopor
{
	#[allow(clippy::new_ret_no_self)] // The mock is only ever used as a solver instance
	fn new(rets: &[TToporReturnVal], vals: &[TToporLitVal]) -> Box<dyn TToporInst>
	{
		Box::new(CMockTopor { m_Rets: rets.to_vec(), m_Vals: vals.to_vec(), m_Solves: 0, m_Clss: 0 })
	}
}

impl TToporInst for CMockTopor
{
	fn ToporSetParam(&mut self, _paramName: &str, _newVal: f64) {}
	fn ToporIsError(&self) -> bool
	{
		false
	}
	fn ToporGetStatusExplanation(&self) -> String
	{
		format!("mock: {} clauses", self.m_Clss)
	}
	fn ToporDumpDrat(&mut self, _openedDratFile: Box<dyn Write>, _isDratBinary: bool, _dratSortEveryClause: bool) {}
	fn ToporGetLitValue(&self, l: TLit) -> TToporLitVal
	{
		let v = self.m_Vals[std::cmp::min(self.m_Solves, self.m_Vals.len()) - 1];
		if l > 0
		{
			v
		}
		else if v == TToporLitVal::VAL_SATISFIED
		{
			TToporLitVal::VAL_UNSATISFIED
		}
		else
		{
			TToporLitVal::VAL_SATISFIED
		}
	}
	fn ToporClearUserPolarityInfo(&mut self, _v: TLit) {}
	fn ToporFixPolarity(&mut self, _l: TLit, _onlyOnce: bool) {}
	fn ToporCreateInternalLit(&mut self, _v: TLit) {}
	fn ToporBoostScore(&mut self, _v: TLit, _value: f64) {}
	fn ToporBacktrack(&mut self, _decLevel: TLit) {}
	fn ToporChangeConfigToGiven(&mut self, _configNum: u16) -> String
	{
		String::new()
	}
	fn ToporGetLitDecLevel(&self, _l: TLit) -> TLit
	{
		0
	}
	fn ToporSolve(&mut self, _assumps: &[TLit], _toInSecIsCpuTime: (f64, bool), _confThr: u64) -> TToporReturnVal
	{
		self.m_Solves += 1;
		self.m_Rets[std::cmp::min(self.m_Solves, self.m_Rets.len()) - 1]
	}
	fn ToporAddClause(&mut self, _c: &[TLit])
	{
		self.m_Clss += 1;
	}
	fn ToporGetSolveInvs(&self) -> u64
	{
		0
	}
	fn ToporIsAssumptionRequired(&mut self, _assumpInd: usize) -> bool
	{
		true
	}
	fn ToporGetMaxUserVar(&self) -> TLit
	{
		1
	}
	fn ToporHintPhase(&mut self, _l: TLit) {}
	fn ToporHintScore(&mut self, _v: TLit, _score: f64) {}
	fn ToporReadHints(&mut self, _text: &str) -> Result<(usize, usize), String>
	{
		Ok((0, 0))
	}
	fn ToporWriteHints(&self) -> String
	{
		String::new()
	}
}

// Runs the given function on a tool, which hasn't read anything yet; returns the result of the function and the output of the tool
fn WithTool<R>(f: impl FnOnce(&mut CToporTool) -> R) -> (R, String)
{
	let argv: Vec<String> = vec![];
	let mut out: Vec<u8> = vec![];
	let r =
	{
		let mut tool = CToporTool::new(&argv, &mut out);
		f(&mut tool)
	};
	(r, String::from_utf8_lossy(&out).into_owned())
}

#[test]
fn tool_with_wrong_solver_answers()
{
	use TToporLitVal::*;
	use TToporReturnVal::*;

	// No solver yet
	let (r, out) = WithTool(|tool|
	{
		assert!(tool.AllToporsNull());
		(tool.ToporOnFinishedSolving(&[], None), tool.OnInputCompleted())
	});
	assert_eq!((r, out.as_str()), ((BadRetVal, BadRetVal), ""));

	// The mock itself behaves as the tool expects
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_SATISFIED]));
		assert!(!tool.AllToporsNull());
		let mut l = CLine::new(b"r /a 1\n".to_vec());
		assert_eq!(tool.HandleLine(&mut l), Ok(None));
		for line in ["lb 1 1\n", "lf 1 0\n", "lc 1\n", "ll 1\n", "b 0\n", "n 1\n", "1 0\n"]
		{
			assert_eq!(tool.HandleLine(&mut CLine::new(line.as_bytes().to_vec())), Ok(None));
		}
		assert!(!tool.Topor().ToporIsError());
		assert_eq!(tool.Topor().ToporGetLitDecLevel(1), 0);
		tool.Topor().ToporDumpDrat(Box::new(CFailedOfstream), true, false);
		tool.verifyModel = true;
		tool.Solve(Some(&[1, -2, 0]))
	});
	assert_eq!(r, BadRetVal);
	// The second assumption is falsified by the "model"
	assert_eq!(out, "c converted configuration number 1 to parameters \ns SATISFIABLE\nv 1 0\nc mock: 1 clauses\nc topor_tool: before verifying that the model satisfies the assumptions and the clauses\nc ERROR: assumptions -2 is not satisfied!\n");

	// A model, which falsifies a clause
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_UNSATISFIED]));
		tool.verifyModel = true;
		tool.vmClss = vec![vec![-1, 2, 0], vec![1, 3, 0], vec![-4, 0]];
		(tool.Solve(None), tool.retValBasedOnLatestSolve)
	});
	assert_eq!(r, (BadRetVal, 10));
	assert_eq!(out, "s SATISFIABLE\nv -1 0\nc mock: 0 clauses\nc topor_tool: before verifying that the model satisfies the clauses\nc ERROR: the following clause is not satisfied: 1 3 0\n");

	// The same at the end of the input
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_UNSATISFIED]));
		tool.verifyModel = true;
		tool.vmClss = vec![vec![1, 0]];
		tool.OnInputCompleted()
	});
	assert_eq!(r, BadRetVal);
	assert!(out.ends_with("c ERROR: the following clause is not satisfied: 1 0\n"), "{}", out);

	// A don't care literal satisfies a clause and an assumption
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_DONT_CARE]));
		tool.verifyModel = true;
		tool.vmClss = vec![vec![1, 0]];
		tool.Solve(Some(&[1]))
	});
	assert_eq!(r, 10);
	assert!(out.ends_with("c topor_tool: assumptions verified!\nc topor_tool: clauses verified!\n"), "{}", out);

	// An unsatisfiable core, which isn't unsatisfiable
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_UNSAT, RET_SAT], &[VAL_SATISFIED]));
		tool.verifyUcore = true;
		tool.printUcore = true;
		tool.Solve(Some(&[1, -2, 0, 3]))
	});
	assert_eq!(r, BadRetVal);
	assert_eq!(out, "s UNSATISFIABLE\nv 1 -2 0 3 0\nc mock: 0 clauses\nAssumption #0 -- 1 : 1\nAssumption #1 -- -2 : 1\ns SATISFIABLE\nv 1 0\nc mock: 0 clauses\nret == 10: UNSAT CORE BUG!!!!!\n");

	// The solver fails, while the core is being verified
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_UNSAT, RET_TIMEOUT_GLOBAL], &[VAL_SATISFIED]));
		tool.verifyUcore = true;
		tool.Solve(None)
	});
	assert_eq!(r, BadRetVal);
	assert_eq!(out, "s UNSATISFIABLE\nc mock: 0 clauses\ns TIMEOUT_GLOBAL\nc mock: 0 clauses\nret == -1: UNSAT CORE BUG!!!!!\n");

	// AllSAT: the first model falsifies a clause
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_UNSATISFIED]));
		tool.verifyModel = true;
		tool.allsatModels = 3;
		tool.blockingVars = vec![1, 2];
		tool.vmClss = vec![vec![1, 0]];
		tool.OnInputCompleted()
	});
	assert_eq!(r, BadRetVal);
	assert_eq!(out, "s SATISFIABLE\nv -1 -2 0\nc mock: 0 clauses\nc topor_tool: before verifying that the model satisfies the clauses\nc ERROR: the following clause is not satisfied: 1 0\n");

	// AllSAT: the second model falsifies a clause
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT], &[VAL_SATISFIED, VAL_UNSATISFIED]));
		tool.verifyModel = true;
		tool.allsatModels = 3;
		tool.blockingVars = vec![1, 2];
		tool.vmClss = vec![vec![1, 0]];
		tool.OnInputCompleted()
	});
	assert_eq!(r, BadRetVal);
	assert_eq!(out, concat!("s SATISFIABLE\nv 1 2 0\nc mock: 0 clauses\nc topor_tool: before verifying that the model satisfies the clauses\nc topor_tool: clauses verified!\n",
		"c topor_tool: before adding a blocking clause and calling the solver for time 2 out of 3\n",
		"s SATISFIABLE\nv -1 -2 0\nc mock: 1 clauses\nc topor_tool: before verifying that the model satisfies the clauses\nc ERROR: the following clause is not satisfied: 1 0\n"));

	// AllSAT: the solver fails in the middle
	let (r, out) = WithTool(|tool|
	{
		tool.topor = Some(CMockTopor::new(&[RET_SAT, RET_SAT, RET_MEM_OUT], &[VAL_SATISFIED, VAL_UNSATISFIED]));
		tool.allsatModels = 10;
		tool.blockingVars = vec![1];
		tool.OnInputCompleted()
	});
	assert_eq!(r, BadRetVal);
	assert_eq!(out, concat!("s SATISFIABLE\nv 1 0\nc mock: 0 clauses\n",
		"c topor_tool: before adding a blocking clause and calling the solver for time 2 out of 10\ns SATISFIABLE\nv -1 0\nc mock: 1 clauses\n",
		"c topor_tool: before adding a blocking clause and calling the solver for time 3 out of 10\ns MEMORY_OUT\nc mock: 2 clauses\n"));
}

/*
* Archives and input files, which cannot be read
*/

#[test]
fn identify_archive_file_type()
{
	// The compile-time test of the signatures
	assert!(AllSigsDifferByFirstInt());
	assert_eq!(archiveCommands.len(), fileSig.len());

	let I = |bytes: &[u8]| -> (TArchiveFileType, usize)
	{
		let mut i = 0;
		let aFileType = IdentifyArchiveFileType(&mut ||
		{
			i += 1;
			if i <= bytes.len() { bytes[i - 1] as i32 } else { EOF }
		});
		// The type and the number of the characters read (including the EOF, if any)
		(aFileType, i)
	};

	assert_eq!(I(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00, 0x04]).0, TArchiveFileType::XZ);
	assert_eq!(I(&[0x5D, 0x00, 0x00, 0x80, 0x00, 0xFF, 0xFF]).0, TArchiveFileType::LZMA);
	assert_eq!(I(&[0x42, 0x5A, 0x68, 0x39]).0, TArchiveFileType::BZ);
	assert_eq!(I(&[0x1F, 0x8B, 0x08]).0, TArchiveFileType::GZ);
	assert_eq!(I(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00]).0, TArchiveFileType::SevenZ);
	// The signature is the whole file
	assert_eq!(I(&[0x1F, 0x8B]), (TArchiveFileType::GZ, 3));
	// The file ends in the middle of the signature: it's considered to be an archive, precisely as in the C++ version
	assert_eq!(I(&[0x1F]), (TArchiveFileType::GZ, 2));
	assert_eq!(I(&[0xFD, 0x37, 0x7A]), (TArchiveFileType::XZ, 4));
	// Plain text
	assert_eq!(I(b"p cnf 3 2\n"), (TArchiveFileType::None, 1));
	assert_eq!(I(b""), (TArchiveFileType::None, 1));
	// The first character only matches
	assert_eq!(I(&[0x1F, 0x00, 0x8B]), (TArchiveFileType::None, 2));
	assert_eq!(I(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x01]).0, TArchiveFileType::None);
	// The first character of '7' matches the 7z signature
	assert_eq!(I(b"7 8 0\n"), (TArchiveFileType::None, 2));
	// A mismatch, whose character starts the signature of a later type
	assert_eq!(I(&[0xFD, 0x1F, 0x8B, 0x08]).0, TArchiveFileType::GZ);
	assert_eq!(I(&[0x1F, 0x42, 0x5A, 0x68]).0, TArchiveFileType::None);
}

#[test]
fn archives()
{
	// A file with the signature of 7z, which is not an archive: whether 7z exists or not, the pipe provides nothing
	let f = TmpFile("not_an_archive.7z");
	std::fs::write(&f, [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x04, 0x01, 0x02, 0x03]).unwrap();
	let (res, out) = RunTool(&[&f, "/topor_tool/print_model", "1"]);
	assert_eq!(res, Ok(BadRetVal), "{}", out);
	assert_eq!(out, format!("c Intel(R) SAT Solver started\nc topor_tool: file type determined to an archive file. The following command will be used to read it through a pipe : 7z x -so {} 2>/dev/null\n", f));
	let _ = std::fs::remove_file(&f);

	// A real gzip archive, if gzip exists on the machine
	let cnf = TmpFile("archive.cnf");
	let gz = TmpFile("archive.cnf.gz");
	std::fs::write(&cnf, "c gzipped\np cnf 3 3\n1 2 0\n-1 0\n-2 3 0\ns -3 0\ns 0\n").unwrap();
	let gzipped = match std::process::Command::new("gzip").arg("-c").arg(&cnf).stderr(std::process::Stdio::null()).output()
	{
		Ok(output) if output.status.success() && output.stdout.starts_with(&[0x1F, 0x8B]) => Some(output.stdout),
		_ => None,
	};
	let _ = std::fs::remove_file(&cnf);
	if let Some(gzipped) = gzipped
	{
		std::fs::write(&gz, gzipped).unwrap();
		let (res, out) = RunTool(&[&gz, "/topor_tool/print_ucore", "1"]);
		let _ = std::fs::remove_file(&gz);
		assert_eq!(res, Ok(10), "{}", out);
		assert_eq!(out, format!(concat!("c Intel(R) SAT Solver started\nc topor_tool: file type determined to an archive file. The following command will be used to read it through a pipe : gzip -c -d {} \n",
			"c topor_tool: suggested #variables : 3; suggested #clauses : 3\nc /topor_tool/print_ucore 1\ns UNSATISFIABLE\nv -3 0\ns SATISFIABLE\nv -1 2 3 0\n"), gz));
	}
}

#[cfg(unix)]
#[test]
fn unreadable_input_files()
{
	use std::os::unix::fs::PermissionsExt;

	// A file without the read permission
	let f = TmpFile("unreadable.cnf");
	std::fs::write(&f, "1 0\n").unwrap();
	std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
	// The super-user can read anything
	let canOpenAnything = std::fs::File::open(&f).is_ok();
	if !canOpenAnything
	{
		let (res, out) = RunTool(&[&f]);
		assert_eq!(res, Ok(BadRetVal));
		assert_eq!(out, format!("c Intel(R) SAT Solver started\nc topor_tool ERROR: couldn't open the input file {} to verify the signature\n", f));
	}
	let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600));
	let _ = std::fs::remove_file(&f);

	// A file, which can be opened to verify the signature, but cannot be opened afterwards: a named pipe, whose read permission is removed
	// by its writer after the tool had opened it for the first time and before the tool can read anything from it (so the order of the events is fixed)
	let fifo = TmpFile("unreadable_later.cnf");
	let _ = std::fs::remove_file(&fifo);
	let isFifoCreated = matches!(std::process::Command::new("mkfifo").arg(&fifo).stderr(std::process::Stdio::null()).status(), Ok(status) if status.success());
	if isFifoCreated && !canOpenAnything
	{
		let fifoWriter = fifo.clone();
		let writer = std::thread::spawn(move ||
		{
			// Blocked until the tool opens the pipe
			let mut w = std::fs::OpenOptions::new().write(true).open(&fifoWriter).unwrap();
			std::fs::set_permissions(&fifoWriter, std::fs::Permissions::from_mode(0o200)).unwrap();
			let _ = w.write_all(b"1 0\n");
		});
		let (res, out) = RunTool(&[&fifo]);
		writer.join().unwrap();
		assert_eq!(res, Ok(BadRetVal));
		assert_eq!(out, "c Intel(R) SAT Solver started\nc topor_tool ERROR: couldn't open the input file\n");
	}
	let _ = std::fs::remove_file(&fifo);
}

/*
* main itself: the test executable runs itself, so that the only test of the child process is main_child, which invokes main with the command line of the child process
* The command line must be meaningful for both the test harness (the arguments are test filters, where only main_child may match) and for the tool (the input file and the parameters)
*/

const MainChildEnvVar: &str = "INTEL_SAT_SOLVER_TEST_MAIN_CHILD";
const MainChildTest: &str = "main_tests::main_child";

// The uncaught error path of main ends with abort: the child process turns the abort signal into an ordinary exit with the conventional exit code,
// so that whatever is done on exit is still done (in particular, the coverage profile of the child process is written, if the coverage is being measured)
#[cfg(unix)]
extern "C" fn MainChildOnAbort(_sig: i32)
{
	std::process::exit(128 + 6);
}

#[test]
fn main_child()
{
	if std::env::var_os(MainChildEnvVar).is_some()
	{
		#[cfg(unix)]
		{
			extern "C"
			{
				fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
			}
			// SIGABRT
			unsafe { signal(6, MainChildOnAbort) };
		}
		main();
	}
}

fn MainChildCommand(inputFile: &str) -> std::process::Command
{
	let mut command = std::process::Command::new(std::env::current_exe().unwrap());
	command.env(MainChildEnvVar, "1").current_dir(std::path::Path::new(inputFile).parent().unwrap()).arg(inputFile).stdin(std::process::Stdio::null());
	command
}

#[test]
fn main_in_child_process()
{
	// A satisfiable instance: the name of the test is the name of the DRAT file (in the temporary directory)
	let f = TmpFile("main_child_sat.cnf");
	std::fs::write(&f, "p cnf 2 2\n1 2 0\n-1 0\n").unwrap();
	let output = MainChildCommand(&f).args(["/topor_tool/text_drat_file", MainChildTest]).output().unwrap();
	let out = String::from_utf8_lossy(&output.stdout).into_owned();
	assert_eq!(output.status.code(), Some(10), "{}\n{}", out, String::from_utf8_lossy(&output.stderr));
	assert!(out.contains(&format!("c Intel(R) SAT Solver started\nc topor_tool: suggested #variables : 2; suggested #clauses : 2\nc /topor_tool/text_drat_file {}\ns SATISFIABLE\nv -1 2 0\n", MainChildTest)), "{}", out);
	let _ = std::fs::remove_file(&f);
	let _ = std::fs::remove_file(TmpFile(MainChildTest));

	// An unsatisfiable instance and a wrong parameter
	let f = TmpFile("main_child_unsat.cnf");
	std::fs::write(&f, "1 0\n-1 0\n").unwrap();
	let output = MainChildCommand(&f).args(["/topor_tool/text_drat_file", MainChildTest]).output().unwrap();
	assert_eq!(output.status.code(), Some(20));
	assert!(String::from_utf8_lossy(&output.stdout).contains("s UNSATISFIABLE\n"));
	let _ = std::fs::remove_file(TmpFile(MainChildTest));
	let output = MainChildCommand(&f).args([MainChildTest, "--nocapture"]).output().unwrap();
	assert_eq!(output.status.code(), Some(BadRetVal & 0xFF));
	assert!(String::from_utf8_lossy(&output.stdout).contains("c topor_tool ERROR: could not convert --nocapture to double\n"));
	let _ = std::fs::remove_file(&f);

	// An archive, where the decompression program cannot be found: as with popen, the shell reports it and the input is empty
	let f = TmpFile("main_child_archive.cnf.gz");
	std::fs::write(&f, [0x1F, 0x8B, 0x08, 0x00]).unwrap();
	let output = MainChildCommand(&f).env("PATH", TmpFile("no_such_dir")).args([MainChildTest, "--nocapture"]).output().unwrap();
	let out = String::from_utf8_lossy(&output.stdout).into_owned();
	assert_eq!(output.status.code(), Some(BadRetVal & 0xFF), "{}", out);
	assert!(out.contains(&format!("The following command will be used to read it through a pipe : gzip -c -d {} \n", f)), "{}", out);
	assert!(!out.contains("couldn't open the input file"), "{}", out);
	assert!(String::from_utf8_lossy(&output.stderr).contains("sh: gzip: command not found\n"));
	let _ = std::fs::remove_file(&f);

	// An error, which the C++ version reports by throwing an exception out of main: the process is aborted
	let f = TmpFile("main_child_abort.cnf");
	std::fs::write(&f, "ot 5 1\nox\n").unwrap();
	let mut command = MainChildCommand(&f);
	command.args([MainChildTest, "--nocapture"]);
	let output = command.output().unwrap();
	let out = String::from_utf8_lossy(&output.stdout).into_owned();
	let err = String::from_utf8_lossy(&output.stderr).into_owned();
	assert!(!output.status.success(), "{}\n{}", out, err);
	// See MainChildOnAbort
	#[cfg(unix)]
	assert_eq!(output.status.code(), Some(128 + 6), "{}\n{}", out, err);
	assert!(out.contains("c Intel(R) SAT Solver started\n"), "{}\n{}", out, err);
	assert!(err.contains("terminate called after throwing an instance of 'std::logic_error'\n  what():  c topor_tool ERROR: The 2nd character must be either t or c at line number 2\n"), "{}\n{}", out, err);
	let _ = std::fs::remove_file(&f);
}

/*
* The IPASIR interface of the library, as linked into the executable
*/

#[test]
fn ipasir_of_the_linked_library()
{
	use intel_sat_solver::topor_ipasir::*;
	use std::ffi::{c_int, c_void, CStr, CString};

	unsafe extern "C" fn Terminate(state: *mut c_void) -> c_int
	{
		let calls = &mut *(state as *mut c_int);
		*calls += 1;
		1
	}

	assert_eq!(unsafe { CStr::from_ptr(ipasir_signature()) }.to_str().unwrap(), "IntelSatSolver");
	unsafe
	{
		ipasir_release(std::ptr::null_mut());
		let solver = ipasir_init();
		assert!(!solver.is_null());
		let paramName = CString::new("/verbosity/level").unwrap();
		let paramVal = CString::new("0").unwrap();
		ipasir_set_parameter(solver, paramName.as_ptr(), paramVal.as_ptr());
		for l in [1, 2, 0, -1, 2, 0, -2, 3, 0]
		{
			ipasir_add(solver, l);
		}
		// Unassigned before the first query
		assert_eq!(ipasir_val(solver, 2), 0);
		assert_eq!(ipasir_solve(solver), 10);
		assert_eq!((ipasir_val(solver, 2), ipasir_val(solver, -2), ipasir_val(solver, 3), ipasir_val(solver, -3)), (2, 2, 3, 3));
		ipasir_assume(solver, 2);
		ipasir_assume(solver, -3);
		ipasir_assume(solver, 2);
		assert_eq!(ipasir_solve(solver), 20);
		assert_eq!(ipasir_failed(solver, -3), 1);
		assert_eq!(ipasir_failed(solver, 2), 1);
		// The solver is stopped by the terminate callback, until it's removed
		let mut calls: c_int = 0;
		ipasir_add(solver, 4);
		ipasir_add(solver, 5);
		ipasir_add(solver, 0);
		ipasir_set_terminate(solver, &mut calls as *mut c_int as *mut c_void, Some(Terminate));
		assert_eq!(ipasir_solve(solver), 0);
		assert_eq!(calls, 1);
		ipasir_set_terminate(solver, std::ptr::null_mut(), None);
		assert_eq!(ipasir_solve(solver), 10);
		assert_eq!(calls, 1);
		ipasir_release(solver);
	}

	// The wrapper class
	let mut tiw = CToporIpasirWrapper::default();
	tiw.SetParam("/verbosity/level", "0");
	tiw.Add(-1);
	tiw.Add(0);
	assert_eq!(tiw.Solve(), 10);
	assert_eq!((tiw.Val(1), tiw.Val(-1), tiw.Val(9)), (-1, -1, 9));
	tiw.Assume(1);
	assert_eq!(tiw.Solve(), 20);
	assert_eq!(tiw.Failed(1), 1);
	assert_eq!(tiw.Topor().GetSolveInvs(), 2);
}
