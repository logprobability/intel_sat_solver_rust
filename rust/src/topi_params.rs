// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::color_print::{ansi_color_code, print_as_color};
use crate::topor_dyn_array::MultiplierDef;
use std::collections::{BTreeMap, HashMap};

// The number of parameter modes
pub type TModeType = u8;
#[allow(non_upper_case_globals)]
pub const m_Modes: TModeType = 9;
#[allow(non_upper_case_globals)]
pub const m_ModeParamName: &str = "/mode/value";

// variant<double, TModeType> newValOrNewMode
#[derive(Clone, Copy)]
pub enum TValOrMode
{
	Val(f64),
	Mode(TModeType),
}

// An arithmetic type, which can serve as a parameter:
// a floating-point type must fit into a double, and an integer-type must fit into half-a-double
pub trait TParamType: Copy + PartialOrd + Default
{
	const IS_FLOATING_POINT: bool;
	// typeid(T).name() for gcc and clang
	fn type_name() -> &'static str;
	fn to_std_string(self) -> String;
	fn from_double(d: f64) -> Self;
	fn to_double(self) -> f64;
	fn limits_min() -> Self;
	fn limits_lowest() -> Self;
	fn limits_max() -> Self;
	fn limits_epsilon() -> Self;
}

macro_rules! impl_param_type_int
{
	($t:ty, $name:expr) =>
	{
		impl TParamType for $t
		{
			const IS_FLOATING_POINT: bool = false;
			fn type_name() -> &'static str { $name }
			fn to_std_string(self) -> String { self.to_string() }
			fn from_double(d: f64) -> Self { d as $t }
			fn to_double(self) -> f64 { self as f64 }
			fn limits_min() -> Self { <$t>::MIN }
			fn limits_lowest() -> Self { <$t>::MIN }
			fn limits_max() -> Self { <$t>::MAX }
			fn limits_epsilon() -> Self { 0 }
		}
	};
}

impl_param_type_int!(u8, "h");
impl_param_type_int!(u16, "t");
impl_param_type_int!(u32, "j");

macro_rules! impl_param_type_float
{
	($t:ty, $name:expr) =>
	{
		impl TParamType for $t
		{
			const IS_FLOATING_POINT: bool = true;
			fn type_name() -> &'static str { $name }
			fn to_std_string(self) -> String { if self.is_nan() { "nan".to_string() } else { format!("{:.6}", self) } }
			fn from_double(d: f64) -> Self { d as $t }
			fn to_double(self) -> f64 { self as f64 }
			fn limits_min() -> Self { <$t>::MIN_POSITIVE }
			fn limits_lowest() -> Self { <$t>::MIN }
			fn limits_max() -> Self { <$t>::MAX }
			fn limits_epsilon() -> Self { <$t>::EPSILON }
		}
	};
}

impl_param_type_float!(f64, "d");
impl_param_type_float!(f32, "f");

impl TParamType for bool
{
	const IS_FLOATING_POINT: bool = false;
	fn type_name() -> &'static str { "b" }
	fn to_std_string(self) -> String { (self as u8).to_string() }
	fn from_double(d: f64) -> Self { d != 0. }
	fn to_double(self) -> f64 { self as u8 as f64 }
	fn limits_min() -> Self { false }
	fn limits_lowest() -> Self { false }
	fn limits_max() -> Self { true }
	fn limits_epsilon() -> Self { false }
}

pub const ERR_VAL_BELOW_MIN: &str = "the value is below the minimal value";
pub const ERR_VAL_ABOVE_MAX: &str = "the value is above the maximal value";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum TErrorType
{
	NO_ERR = 0,
	NAME_EXISTS_WHEN_CREATING_PARAMETER = 1,
	NAME_DOESNT_EXIST_WHEN_SETTING_PARAMETER = 2,
	ERROR_WHEN_SETTING_PARAMETER = 3,
}

// One parameter; holds the value and whatever is required to update it (which is captured by the update lambda in the C++ version)
#[derive(Clone, Copy, Default)]
pub struct CTopiParam<T: TParamType>
{
	m_Val: T,
	m_MinVal: T,
	m_MaxVal: T,
	// The initial value for each mode, if the parameter is mode-dependent
	m_InitValsPerMode: Option<[T; m_Modes as usize]>,
}

impl<T: TParamType> CTopiParam<T>
{
	// The value of the parameter (operator T() in the C++ version)
	#[inline(always)]
	pub fn get(&self) -> T
	{
		self.m_Val
	}

	// Assign the value of the parameter directly (the assignment m_ParamA = m_ParamB in the C++ version, which copies the value only)
	#[inline(always)]
	pub fn set(&mut self, v: T)
	{
		self.m_Val = v;
	}

	// CTopiParam semantics: <type, initVal, minVal, maxVal>, where minVal <= initVal <= maxVal
	// Returns the parameter and its updated description (or None, if there is an error)
	pub(crate) fn new_single(err: &mut dyn FnMut(TErrorType, &str), is_error_already: bool, descr: &str, init_val: T, min_val: Option<T>, max_val: Option<T>) -> (Self, Option<String>)
	{
		let min_val = min_val.unwrap_or(T::limits_lowest());
		let max_val = max_val.unwrap_or(T::limits_max());
		let param = CTopiParam { m_Val: init_val, m_MinVal: min_val, m_MaxVal: max_val, m_InitValsPerMode: None };

		let IsError = Self::AssertMinMax(err, init_val, min_val, max_val) || is_error_already;

		if IsError
		{
			return (param, None);
		}

		let updated_descr = T::type_name().to_string() + "; default = " + &print_as_color(Self::Val2Str(init_val) + " in [" + &Self::Val2Str(min_val) + ", " + &Self::Val2Str(max_val) + "]", ansi_color_code::green) + " : " + descr;

		(param, Some(updated_descr))
	}

	pub(crate) fn new_modes(err: &mut dyn FnMut(TErrorType, &str), is_error_already: bool, descr: &str, init_vals_per_mode: [T; m_Modes as usize], min_val: Option<T>, max_val: Option<T>) -> (Self, Option<String>)
	{
		let min_val = min_val.unwrap_or(T::limits_min());
		let max_val = max_val.unwrap_or(T::limits_max());
		// Picking the value of the default mode, since any mode change is expected to occur later anyways
		let param = CTopiParam { m_Val: init_vals_per_mode[0], m_MinVal: min_val, m_MaxVal: max_val, m_InitValsPerMode: Some(init_vals_per_mode) };

		let mut IsError = is_error_already;
		for init_val in init_vals_per_mode
		{
			IsError = Self::AssertMinMax(err, init_val, min_val, max_val) || IsError;
		}

		if IsError
		{
			return (param, None);
		}

		let updated_descr = T::type_name().to_string() + "; default = " + &print_as_color(Self::Vals2Str(&init_vals_per_mode) + " in [" + &Self::Val2Str(min_val) + ", " + &Self::Val2Str(max_val) + "]", ansi_color_code::green) + " : " + descr;

		(param, Some(updated_descr))
	}

	fn update(&mut self, new_val_or_new_mode: TValOrMode) -> String
	{
		match new_val_or_new_mode
		{
			// Value
			TValOrMode::Val(new_val) => self.UpdateValue(new_val),
			// Mode: has an impact only if there is a separate initial value for every mode
			TValOrMode::Mode(new_mode) => match self.m_InitValsPerMode
			{
				Some(init_vals_per_mode) => self.UpdateValue(init_vals_per_mode[new_mode as usize].to_double()),
				None => String::new(),
			},
		}
	}

	fn val_str(&self) -> String
	{
		Self::Val2Str(self.m_Val)
	}

	fn Val2Str(any_val: T) -> String
	{
		if !T::IS_FLOATING_POINT
		{
			return any_val.to_std_string();
		}

		if any_val == T::limits_min()
		{
			return "min(".to_string() + T::type_name() + ")";
		}

		if any_val == T::limits_lowest()
		{
			return "lowest(".to_string() + T::type_name() + ")";
		}

		if any_val == T::limits_max()
		{
			return "max(".to_string() + T::type_name() + ")";
		}

		if any_val == T::limits_epsilon()
		{
			return "epsilon(".to_string() + T::type_name() + ")";
		}

		any_val.to_std_string()
	}

	fn Vals2Str(init_vals: &[T; m_Modes as usize]) -> String
	{
		let mut res = "{".to_string();

		for any_val in init_vals
		{
			res += &Self::Val2Str(*any_val);
			res += ", ";
		}

		res.pop();
		res.pop();
		res += "}";

		res
	}

	// Returns true iff there is an error
	fn AssertMinMax(err: &mut dyn FnMut(TErrorType, &str), init_val: T, min_val: T, max_val: T) -> bool
	{
		if init_val < min_val
		{
			err(TErrorType::ERROR_WHEN_SETTING_PARAMETER, ERR_VAL_BELOW_MIN);
			return true;
		}

		if init_val > max_val
		{
			err(TErrorType::ERROR_WHEN_SETTING_PARAMETER, ERR_VAL_ABOVE_MAX);
			return true;
		}

		false
	}

	fn UpdateValue(&mut self, new_val: f64) -> String
	{
		if !T::IS_FLOATING_POINT && new_val.trunc() != new_val
		{
			return "the value is ".to_string() + &new_val.to_std_string() + ", but it must be an integer";
		}

		self.m_Val = T::from_double(new_val);

		if new_val < self.m_MinVal.to_double()
		{
			return "the value ".to_string() + &(new_val as i64).to_string() + " is below the minimal value";
		}

		if new_val > self.m_MaxVal.to_double()
		{
			return "the value ".to_string() + &(new_val as i64).to_string() + " is above the maximal value";
		}

		String::new()
	}
}

// Test-only: makes the definition of the given parameter count as invalid (in the current thread)
#[cfg(test)]
pub mod param_def_fault
{
	use std::cell::Cell;
	thread_local!
	{
		static FAULTY_PARAM: Cell<Option<&'static str>> = const { Cell::new(None) };
	}

	pub fn Set(name: Option<&'static str>)
	{
		FAULTY_PARAM.with(|c| c.set(name));
	}

	pub fn IsFaulty(name: &str) -> bool
	{
		FAULTY_PARAM.with(|c| c.get() == Some(name))
	}
}

type TUpdateFunc = fn(&mut CTopiParams, TValOrMode) -> String;
type TGetValFunc = fn(&CTopiParams) -> String;

macro_rules! opt_expr
{
	() => { None };
	($e:expr) => { Some($e) };
}

// Contains a map from parameter name to parameter description & function to set it for every parameter
// Every parameter adds itself to the list, so we don't have to do it manually
// To add a parameter, it suffices to declare it below, c'est tout!
// The syntax per parameter: field: type = single|modes(name, description, initVal|[initVal per mode] [, minVal [, maxVal]]);
// where the default minVal is lowest(type) for single and min(type) for modes, and the default maxVal is max(type)
macro_rules! topi_params
{
	( $bins:ident; $( $(#[$attr:meta])* $field:ident : $t:ty = $kind:ident ( $name:expr, $descr:expr, $init:expr $(, $min:expr $(, $max:expr)? )? ) ; )* ) =>
	{
		pub struct CTopiParams
		{
			m_ErrorCode: TErrorType,
			m_Mode: TModeType,
			m_ErrorDescr: String,
			// name -> (description, Update, GetVal)
			m_Name2DescrUpdateGetval: BTreeMap<String, (String, TUpdateFunc, TGetValFunc)>,
			m_Name2PrevVal: HashMap<String, String>,
			$( $(#[$attr])* pub $field: CTopiParam<$t>, )*
		}

		impl CTopiParams
		{
			// bins_in_long_bit_ceil is TWatchInfo::BinsInLongBitCeil, which depends on the index type the solver is instantiated with
			#[allow(non_snake_case)]
			pub fn new($bins: u8) -> Self
			{
				let mut p = CTopiParams
				{
					m_ErrorCode: TErrorType::NO_ERR,
					m_Mode: 0,
					m_ErrorDescr: String::new(),
					m_Name2DescrUpdateGetval: BTreeMap::new(),
					m_Name2PrevVal: HashMap::new(),
					$( $field: CTopiParam::default(), )*
				};

				$(
					{
						let is_error_already = p.IsError();
						let mut err_info: Option<(TErrorType, String)> = None;
						let (param, updated_descr) = topi_params!(@ctor $kind, $t, &mut |c: TErrorType, d: &str| err_info = Some((c, d.to_string())), is_error_already, $descr, $init, opt_expr!($($min)?), opt_expr!($($($max)?)?));
						p.$field = param;
						p.OnParamDefined($name, err_info, updated_descr, |p: &mut CTopiParams, x: TValOrMode| p.$field.update(x), |p: &CTopiParams| p.$field.val_str());
					}
				)*

				p
			}
		}
	};
	(@ctor single, $t:ty, $err:expr, $is_err:expr, $descr:expr, $init:expr, $min:expr, $max:expr) =>
	{
		{
			let init: $t = $init;
			let min: Option<$t> = $min;
			let max: Option<$t> = $max;
			CTopiParam::<$t>::new_single($err, $is_err, $descr, init, min, max)
		}
	};
	(@ctor modes, $t:ty, $err:expr, $is_err:expr, $descr:expr, $init:expr, $min:expr, $max:expr) =>
	{
		{
			let init: [$t; m_Modes as usize] = $init;
			let min: Option<$t> = $min;
			let max: Option<$t> = $max;
			CTopiParam::<$t>::new_modes($err, $is_err, $descr, init, min, max)
		}
	};
}

type TUV = u32;
const RESTART_STRAT_NUMERIC: u8 = 0;
const RESTART_STRAT_LBD: u8 = 1;

topi_params!
{
	BinsInLongBitCeil;
	// The special mode meta-parameter; Every parameter can be provided either one single default or a separate default for each mode (in an array)
	m_ModeParam: TModeType = single(m_ModeParamName, "The mode (0: Unweighted MaxSAT; 1: FLA; 2: FSN; 3: MF2S; 4: LAR; 5: Weighted MaxSAT; 6: FRU; 7: SN1, 8: EBB)", 0, 0, m_Modes - 1);

	// Parameters: verbosity
	m_ParamVerbosity: u8 = single("/verbosity/level", "Verbosity level (0: silent; 1: basic statistics; 2: trail&conflict debugging; 3: extensive debugging)", 0, 0, 3);
	m_ParamHeavyVerbosityStartConf: u32 = single("/verbosity/verbosity_start_conf", "Meaningful only if /verbosity/level>1: print-out at level > 1 starting with the provided conflicts number", 0);
	m_ParamStatPrintOutConfs: u32 = single("/verbosity/print_out_confs", "Meaningful only if /verbosity/level>0: the rate of print-outs in #conflicts", 2000, 1);
	m_ParamPrintMemoryProfiling: bool = single("/verbosity/print_mem_prof", "Print-out memory profiling info", false);

	// Parameters: timeout
	m_ParamOverallTimeout: f64 = single("/timeout/global", "An overall global timeout for Topor lifespan: Topor will be unusable after reaching this timeout (note that one can also provide a Solve-specific timeout as a parameter to Solve)", f64::MAX, f64::EPSILON);
	m_ParamOverallTimeoutIsCpu: bool = single("/timeout/global_is_cpu", "Is the overall global timeout (if any) for Topor lifespan CPU (or, otherwise, Wall)", false);

	// Parameters: decision
	m_ParamInitPolarityStrat: u8 = modes("/decision/polarity/init_strategy", "The initial polarity for a new variable: 0: negative; 1: positive; 2: random", [1, 1, 1, 1, 1, 2, 1, 1, 1], 0, 2);
	m_ParamPolarityStrat: u8 = single("/decision/polarity/strategy", "How to set the polarity for a non-forced variable: 0: phase saving; 1: random", 0, 0, 1);
	m_ParamPolarityFlipFactor: u32 = single("/decision/polarity/flip_factor", "If non-0, every N's polarity selection will be flipped", 0);
	m_ParamIfExternalBoostScoreValueOverride: bool = single("/decision/polarity/if_external_boost_score_value_override", "Override the boost value by when BoostScore is invoked by the user?", false);
	m_ParamExternalBoostScoreValueOverride: f64 = single("/decision/polarity/external_boost_score_value_override", "If /decision/polarity/if_external_boost_score_value_override is on, the value by which to override the boost value by when BoostScore is invoked by the user", 1.);
	m_ParamVarActivityInc: f64 = modes("/decision/vsids/var_activity_inc", "Variable activity bumping factor's initial value: m_PosScore[v].m_Score += m_VarActivityInc is carried out for every variable visited during a conflict (hard-coded to 1.0 in Glucose-based solvers)", [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.8, 1.0], 0.0);
	m_ParamVarActivityIncDecay: f64 = modes("/decision/vsids/var_activity_inc_decay", "After each conflict, the variable activity bumping factor is increased by multiplication by 1/m_ParamVarActivityIncDecay (the initial value is provided in the parameter):  m_ParamVarActivityInc *= (1 / m_ParamVarActivityIncDecay), where it's 0.8 in Glucose-based solvers and 0.95 in Minisat", [0.95, 0.8, 0.8, 0.8, 0.8, 0.925, 0.8, 0.95, 0.925], f64::EPSILON, 1.0);
	m_ParamVarActivityIncDecayReinitN: bool = modes("/decision/vsids/var_activity_inc_decay_reinit_n", "Re-initialize /decision/vsids/var_activity_inc_decay before normal incremental Solve?", [true, false, false, false, false, false, false, true, false]);
	m_ParamVarActivityIncDecayReinitS: bool = modes("/decision/vsids/var_activity_inc_decay_reinit_s", "Re-initialize /decision/vsids/var_activity_inc_decay before short incremental Solve?", [true, false, false, false, false, true, false, false, false]);
	m_ParamVarActivityIncDecayStopReinitSInv: u32 = single("/decision/vsids/var_activity_inc_decay_stop_reinit_s_inv", "The first short incremental invocation to stop re-initializing /decision/vsids/var_activity_inc_decay before short incremental Solve (0: never stop; relevant only if /decision/vsids/var_activity_inc_decay_reinit_s=1)", 0);
	m_ParamVarActivityIncDecayStopReinitRestart: u32 = single("/decision/vsids/var_activity_inc_decay_stop_reinit_restart", "The first restart to stop re-initializing /decision/vsids/var_activity_inc_decay before short incremental Solve (0: never stop; relevant only if /decision/vsids/var_activity_inc_decay_reinit_s=1)", 0);
	m_ParamVarActivityIncDecayStopReinitConflict: u32 = single("/decision/vsids/var_activity_inc_decay_stop_reinit_conflict", "The first conflict to stop re-initializing /decision/vsids/var_activity_inc_decay before short incremental Solve (0: never stop; relevant only if /decision/vsids/var_activity_inc_decay_reinit_s=1)", 0);
	m_ParamVarActivityIncDecayStopReinitTime: f64 = single("/decision/vsids/var_activity_inc_decay_stop_reinit_time", "The time in sec. to stop re-initializing /decision/vsids/var_activity_inc_decay before short incremental Solve (0.0: never stop; relevant only if /decision/vsids/var_activity_inc_decay_reinit_s=1)", 0.0);
	m_ParamVarActivityIncDecayReinitVal: f64 = modes("/decision/vsids/var_activity_inc_decay_reinit_val", "For /decision/vsids/var_activity_inc_decay_reinit_each_solve=1: the value to re-initialize /decision/vsids/var_activity_inc_decay before each Solve", [0.8, 0.8, 0.8, 0.8, 0.8, 0.925, 0.8, 0.8, 0.8], f64::EPSILON, 1.0);
	m_ParamVarDecayInc: f64 = modes("/decision/vsids/var_decay_inc", "The increment value for var_activity_inc_decay each var_decay_update_conf_rate conflicts:  m_ParamVarDecay += m_ParamVarDecayInc (hard-coded to 0.01 in Glucose-based solvers)", [0.01, 0.01, 0.01, 0.01, 0.01, 0.04, 0.01, 0.01, 0.01], 0., 1.0);
	m_ParamVarDecayIncAi: f64 = single("/decision/vsids/var_decay_inc_ai", "After initial: the increment value for var_activity_inc_decay each var_decay_update_conf_rate conflicts:  m_ParamVarDecay += m_ParamVarDecayInc (hard-coded to 0.01 in Glucose-based solvers)", 0.01, 0., 1.0);
	m_ParamVarDecayIncS: f64 = modes("/decision/vsids/var_decay_inc_s", "Short incremental: The increment value for var_activity_inc_decay each var_decay_update_conf_rate conflicts:  m_ParamVarDecay += m_ParamVarDecayInc (hard-coded to 0.01 in Glucose-based solvers)", [0.01, 0.01, 0.01, 0.01, 0.01, 0.0025, 0.01, 0.01, 0.01], 0., 1.0);
	m_ParamVarDecayMax: f64 = modes("/decision/vsids/var_decay_max", "The maximal value for var_activity_inc_decay", [0.99, 0.95, 0.9, 0.95, 0.95, 0.98, 0.95, 0.99, 0.9], f64::EPSILON, 1.0);
	m_ParamVarDecayMaxAi: f64 = modes("/decision/vsids/var_decay_max_ai", "After initial: The maximal value for var_activity_inc_decay", [0.99, 0.95, 0.9, 0.95, 0.95, 0.99, 0.95, 0.99, 0.94], f64::EPSILON, 1.0);
	m_ParamVarDecayMaxS: f64 = modes("/decision/vsids/var_decay_max_s", "Short incremental: The maximal value for var_activity_inc_decay", [0.99, 0.95, 0.9, 0.95, 0.95, 0.98, 0.95, 0.99, 0.94], f64::EPSILON, 1.0);
	m_ParamVarDecayUpdateConfRate: u32 = single("/decision/vsids/var_decay_update_conf_rate", "The rate in conflicts of var_decay's update (5000 in Glucose-based solvers)", 5000, 1);
	m_ParamVsidsInitOrder: bool = modes("/decision/vsids/init_order", "The order of inserting into the VSIDS heap (0: bigger indices first; 1: smaller indices first)", [false, true, false, true, true, false, true, false, false]);
	m_ParamVsidsInitOrderAi: bool = modes("/decision/vsids/init_order_ai", "After initial call: the order of inserting into the VSIDS heap (0: bigger indices first; 1: smaller indices first)", [false, true, false, true, true, false, true, true, false]);
	m_ParamVarActivityGlueUpdate: bool = modes("/decision/vsids/var_activity_glue_update", "Do we increase the VSIDS score for every last-level variable, visited during a conflict, whose parent is a learnt clause with LBD score lower than that of the newly learnt clause?", [true, true, true, false, true, true, false, false, true]);
	m_ParamVarActivityUseMapleLevelBreaker: bool = modes("/decision/vsids/var_activity_use_maple_level_breaker", "Maple (MapleLCMDistChronoBT-f2trc-s) multiplies the variable activity bumping factor by 1.5 for variables, whose dec. level is higher-than-or-equal than 2nd highest-1 and by 0.5 for other variables; use it?", [false, false, false, true, true, false, true, false, false]);
	m_ParamVarActivityUseMapleLevelBreakerAi: bool = modes("/decision/vsids/var_activity_use_maple_level_breaker_ai", "After initial call: Maple (MapleLCMDistChronoBT-f2trc-s) multiplies the variable activity bumping factor by 1.5 for variables, whose dec. level is higher-than-or-equal than 2nd highest-1 and by 0.5 for other variables; use it?", [false, false, false, true, true, false, false, false, false]);
	m_ParamVarActivityMapleLevelBreakerDecrease: u32 = modes("/decision/vsids/var_activity_maple_level_breaker_decrease", "If var_activity_use_maple_level_breaker is on, this number is the decrease in the 2nd highest level, so that if the variable is higher-than-or-equal, its bump is more significant", [1, 1, 1, 1, 0, 1, 1, 1, 1]);

	m_ParamInitClssBoostScoreStrat: u8 = modes("/decision/init_clss_boost/strat", "Initial query: variable score boost strategy for user clauses -- 0: none; 1: no clause-size, user-order; 2: clause-size-aware, user-order; 3: no clause-size, reverse-user-order; 4: clause-size-aware, reverse-user-order", [2, 0, 0, 0, 0, 0, 4, 2, 0], 0, 4);
	m_ParamInitClssBoostScoreStratAfterInit: u8 = modes("/decision/init_clss_boost/strat_after_init", "After initial query: variable score boost strategy for user clauses -- 0: none; 1: no clause-size, user-order; 2: clause-size-aware, user-order; 3: no clause-size, reverse-user-order; 4: clause-size-aware, reverse-user-order", [1, 0, 0, 0, 0, 4, 0, 1, 0], 0, 4);

	m_ParamInitClssBoostMultHighest: f64 = modes("/decision/init_clss_boost/mult_highest", "Variable score boost for initial clauses: highest mult value", [10., 10., 10., 10., 10., 10., 10., 5., 10.]);
	m_ParamInitClssBoostMultLowest: f64 = single("/decision/init_clss_boost/mult_lowest", "Variable score boost for initial clauses: lowest mult value", 1.);
	m_ParamInitClssBoostMultDelta: f64 = modes("/decision/init_clss_boost/mult_delta", "Variable score boost for initial clauses: delta mult value", [0.0001, 0.0001, 0.0001, 0.0001, 0.0001, 0.0001, 0.0001, 0.01, 0.0001]);

	m_ParamRandomizePolarityAtEachIncrementalCall: bool = single("/decision/polarity/randomize_at_incremental_calls", "Randomize the polarities of all the variables once at the beginning of each incremental call", false);

	// Parameters: backtracking
	m_ParamChronoBtIfHigherInit: TUV = modes("/backtracking/chrono_bt_if_higher_init", "Initial query: backtrack chronologically, if the decision level difference is higher than the parameter", [100, TUV::MAX, 50, 100, 100, 50, 100, 0, 100]);
	m_ParamChronoBtIfHigherN: TUV = modes("/backtracking/chrono_bt_if_higher_n", "Normal (non-short) incremental query: Backtrack chronologically, if the decision level difference is higher than the parameter", [100, TUV::MAX, 50, 100, 100, 50, 100, 0, 50]);
	m_ParamChronoBtIfHigherS: TUV = modes("/backtracking/chrono_bt_if_higher_s", "Short incremental query: Backtrack chronologically, if the decision level difference is higher than the parameter", [100, TUV::MAX, 50, 100, 100, 50, 100, 100, 50]);

	m_ParamConflictsToPostponeChrono: u32 = modes("/backtracking/conflicts_to_postpone_chrono", "The number of conflicts to postpone considering any backtracking, but NCB", [0, 0, 0, 4000, 8000, 0, 4000, 0, 4000]);
	m_ParamCustomBtStratInit: u8 = modes("/backtracking/custom_bt_strat_init", "Initial query: 0: no custom backtracking; 1, 2: backtrack to the level containing the variable with the best score *instead of any instances of supposed chronological backtracking*, where ties are broken based on the value -- 1: higher levels are preferred; 2: lower levels are preferred ", [2, 0, 0, 0, 1, 0, 0, 0, 0], 0, 2);
	m_ParamCustomBtStratN: u8 = modes("/backtracking/custom_bt_strat_n", "Normal (non-short) incremental query: 0: no custom backtracking; 1, 2: backtrack to the level containing the variable with the best score *instead of any instances of supposed chronological backtracking*, where ties are broken based on the value -- 1: higher levels are preferred; 2: lower levels are preferred ", [0, 0, 0, 0, 1, 0, 0, 0, 0], 0, 2);
	m_ParamCustomBtStratS: u8 = modes("/backtracking/custom_bt_strat_s", "Short incremental query: 0: no custom backtracking; 1, 2: backtrack to the level containing the variable with the best score *instead of any instances of supposed chronological backtracking*, where ties are broken based on the value -- 1: higher levels are preferred; 2: lower levels are preferred ", [2, 0, 0, 0, 1, 0, 0, 0, 0], 0, 2);

	// Parameters: BCP
	m_ParamInitEntriesPerWL: u8 = single("/bcp/init_entries_per_wl", "BCP: the number of initial entries in a watch list", if 4 >= BinsInLongBitCeil { 4 } else { BinsInLongBitCeil }, BinsInLongBitCeil);
	m_ParamBCPWLChoice: u8 = modes("/bcp/wl_choice", "User clause processing: how to choose the watches -- 0: prefer shorter WL; 1: prefer longer WL; 2: disregard WL length", [0, 2, 0, 1, 0, 0, 1, 0, 0], 0, 2);
	m_ParamExistingBinWLStrat: u8 = modes("/bcp/existing_bin_wl_start", "BCP: what to do about duplicate binary clauses -- 0: nothing; 1: boost their VSIDS score; 2: add another copy to the watches; 3: inprocessing (if on) to remove duplicates; 4: inprocessing (if on) to boost their VSIDS score", [1, 1, 1, 1, 2, 1, 1, 1, 1], 0, 4);
	m_ParamBinWLScoreBoostFactor: f64 = modes("/bcp/bin_wl_start_score_boost_factor", "BCP: if /bcp/existing_bin_wl_start=1 or 4, what's the factor for boosting the scores", [1., 1., 1., 1., 1., 1., 1., 0.5, 1.], f64::EPSILON);
	m_ParamBestContradictionStrat: u8 = modes("/bcp/best_contradiction_strat", "BCP's best contradiction strategy: 0: size; 1: glue; 2: first; 3: last", [0, 0, 0, 0, 0, 0, 0, 3, 0], 0, 3);

	// Parameters: Add-user-clause
	m_ParamAddClsRemoveClssGloballySatByLitMinSize: u32 = modes("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size", "Assigned literal strategy: check for literals satisfied at decision level 0 and remove globally satisfied clauses for sizes larger than this parameter", [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 2, u32::MAX]);
	m_ParamAddClsRemoveClssGloballySatByLitMinSizeAi: u32 = modes("/add_user_clause/remove_clss_globally_sat_by_literal_larger_size_ai", "After initial call: assigned literal strategy: check for literals satisfied at decision level 0 and remove globally satisfied clauses for sizes larger than this parameter", [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 5, u32::MAX]);
	m_ParamAddClsAtLevel0: bool = single("/add_user_clause/guarantee_level_0", "Guarantee that adding a user clause always occurs at level 0 (by backtracking to 0 after every solve)", false);

	// Parameters: Query type
	m_ParamShortQueryConfThrInv: u32 = single("/query_type/short_query_conf_thr", "If the conflict threshold is at most this parameter, this invocation is considered to be incremental short", 10000);

	// Parameters: debugging
	m_ParamAssertConsistency: u8 = single("/debugging/assert_consistency", "Debugging only: assert the consistency (0: none; 1: trail; 2: trail and WL's for assigned; 3: trail and WL's for all -- an extremely heavy operation)", 0, 0, 3);
	m_ParamAssertConsistencyStartConf: u32 = single("/debugging/assert_consistency_start_conf", "Debugging only; meaningful only if /debugging/assert_consistency=1: assert the consistency starting with the provided conflicts number", 0);
	m_ParamPrintDebugModelInvocation: u32 = single("/debugging/print_debug_model_invocation", "The number of solver invocation to print the model in debug-model format: intended to be used for internal Topor debugging", 0);
	m_ParamVerifyDebugModelInvocation: u32 = single("/debugging/verify_debug_model_invocation", "The number of solver invocation, when the debug-model is verified: intended to be used for internal Topor debugging", 0);
	m_ParamDontDumpClauses: bool = single("/debugging/dont_dump_clauses", "If dumping is enabled, clauses are not dumped, only directives", false);

	// Parameters: assumptions handling
	m_ParamAssumpsSimpAllowReorder: bool = single("/assumptions/allow_reorder", "Assumptions handling: allow reordering assumptions when filtering", true);
	m_ParamAssumpsIgnoreInGlue: bool = single("/assumptions/ignore_in_glue", "Assumptions handling: ignore assumptions when calculating the glue score (following the SAT'13 paper \"Improving Glucose for Incremental SAT Solving with Assumptions : Application to MUS Extraction\")", false);

	// Parameters: conflict analysis
	m_ParamMinimizeClausesMinisat: bool = single("/conflicts/minimize_clauses", "Conflict analysis: apply deep conflict clause minimization", true);
	m_ParamMinimizeClausesBinMaxSize: u32 = modes("/conflicts/bin_res_max_size", "Conflict analysis: maximal size to apply binary minimization (both this condition and maximal LBD must hold; 30 in Glucose, Fiver, Maple)", [30, 30, 30, 30, 50, 30, 30, 30, 30]);
	m_ParamMinimizeClausesBinMaxLbd: u32 = single("/conflicts/bin_res_max_lbd", "Conflict analysis: maximal LBD to apply binary minimization (both this condition and maximal size must hold; 6 in Glucose, Fiver, Maple)", 6);
	m_ParamFlippedRecordingMaxLbdToRecord: u32 = modes("/conflicts/flipped_recording_max_lbd", "Conflict analysis: record a flipped clause with LBD smaller than or equal to the value of the parameter", [u32::MAX, u32::MAX, u32::MAX, 0, 40, u32::MAX, 0, 90, u32::MAX]);
	m_ParamFlippedRecordDropIfSubsumed: bool = single("/conflicts/flipped_drop_if_subsumed", "Conflict analysis: test and drop the flipped clause, if subsumed by the main clause", true);
	m_ParamOnTheFlySubsumptionContradictingMinGlueToDisable: u32 = modes("/conflicts/on_the_fly_subsumption/contradicting_min_glue_to_disable", "Conflict analysis: the minimal glue (LBD) to disable on-the-fly subsumption during conflict analysis over the contradicting clause (1: apply only over initial clauses)", [8, 0, 0, 0, 0, 8, 0, 8, 0]);
	m_ParamOnTheFlySubsumptionParentMinGlueToDisable: u32 = modes("/conflicts/on_the_fly_subsumption/parent_min_glue_to_disable", "Conflict analysis: the minimal glue (LBD) to disable on-the-fly subsumption during conflict analysis over the parents (1: apply only over initial clauses)", [30, 0, 0, 0, 0, 1, 0, 30, 16]);
	m_ParamOnTheFlySubsumptionContradictingFirstRestart: u32 = single("/conflicts/on_the_fly_subsumption/contradicting_first_restart", "Conflict analysis: the first restart after which on-the-fly subsumption over the contradicting clause is enabled", 1);
	m_ParamOnTheFlySubsumptionParentFirstRestart: u32 = single("/conflicts/on_the_fly_subsumption/parent_first_restart", "Conflict analysis: the first restart after which on-the-fly subsumption over the parent clause is enabled", 1);
	m_ParamAllUipMode: u8 = single("/conflicts/all_uip/mode", "Conflict analysis: ALL-UIP scheme mode -- 0: disabled; 1: only main clause; 2: only flipped clause; 3: both main and flipped", 0, 0, 3);
	m_ParamAllUipFirstRestart: u32 = single("/conflicts/all_uip/first_restart", "Conflict analysis: the first restart after which the ALL-UIP scheme is enabled", 0);
	m_ParamAllUipLastRestart: u32 = single("/conflicts/all_uip/last_restart", "Conflict analysis: the last restart after which the ALL-UIP scheme is enabled", u32::MAX);
	m_ParamAllUipFailureThr: f64 = single("/conflicts/all_uip/success_rate_failure_thr", "Conflict analysis: the threshold on ALL-UIP scheme failure (0.8 in the SAT'20 paper)", 0.8);

	// Parameters: restart strategy
	// Restart strategy: initial query
	m_ParamRestartStrategyInit: u8 = modes("/restarts/strategy_init", "Restart strategy for the initial query: 0: numeric (arithmetic, luby or in/out); 1: LBD-average-based", [RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_NUMERIC], 0, 1);
	// Restart strategy: normal incremental query
	m_ParamRestartStrategyN: u8 = modes("/restarts/strategy_n", "Restart strategy for the normal (non-short) incremental query: 0: arithmetic; 1: LBD-average-based", [RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_NUMERIC], 0, 1);
	// Restart strategy: short incremental query
	m_ParamRestartStrategyS: u8 = modes("/restarts/strategy_s", "Restart strategy for the short incremental query: 0: arithmetic; 1: LBD-average-based", [RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_LBD, RESTART_STRAT_NUMERIC, RESTART_STRAT_NUMERIC], 0, 1);

	m_ParamRestartNumericLocal: bool = single("/restarts/numeric/is_local", "Restarts, numeric strategies: use local restarts?", true);
	m_ParamRestartNumericInitConfThr: u32 = modes("/restarts/numeric/conflict_thr", "Restarts, numeric strategy: the initial value for the conflict threshold, which triggers a restart", [1000, 1000, 1000, 1000, 100, 1000, 1000, 1000, 1000], 1);
	m_ParamRestartNumericSubStrat: u8 = single("/restarts/numeric/sub_strat", "Restarts, numeric sub-strategy: 0: arithmetic; 1: luby", 0, 0, 1);
	m_ParamRestartArithmeticConfIncr: u32 = modes("/restarts/arithmetic/conflict_increment", "Restarts, arithmetic numeric strategy: increment value for conflicts", [0, 50, 50, 50, 100, 0, 50, 1, 50]);
	m_ParamRestartLubyConfIncr: f64 = modes("/restarts/luby/conflict_increment", "Restarts, luby numeric strategy: increment value for conflicts", [1.5, 1.5, 1.5, 2., 1.5, 1.5, 2., 1.5, 1.5], 1. + f64::EPSILON);

	m_ParamRestartLbdWinSize: u32 = modes("/restarts/lbd/win_size", "Restart, LBD-average-based strategy: window size", [50, 50, 50, 50, 25, 50, 50, 50, 50], 1);
	m_ParamRestartLbdThresholdGlueVal: u32 = modes("/restarts/lbd/thr_glue_val", "Restart, LBD-average-based strategy: the maximal value for LBD queue update", [u32::MAX, u32::MAX, u32::MAX, 50, 50, u32::MAX, 50, u32::MAX, u32::MAX]);
	m_ParamRestartLbdAvrgMult: f64 = modes("/restarts/lbd/average_mult", "Restarts, LBD-average-based strategy: the multiplier in the formula for determining whether it's time to restart: recent-LBD-average * multiplier > global-LBD-average", [0.8, 0.8, 0.8, 0.8, 0.9, 0.8, 0.8, 0.8, 0.8]);

	m_ParamRestartLbdBlockingEnable: bool = modes("/restarts/lbd/blocking/enable", "Restarts, LBD-average-based blocking enable", [true, true, false, false, true, true, false, true, false]);
	m_ParamRestartLbdBlockingConfsToConsider: u32 = modes("/restarts/lbd/blocking/conflicts_to_consider_blocking", "Restarts, LBD-average-based blocking strategy: the number of conflicts to consider blocking; relevant, if /restarts/lbd/blocking/enable=1", [10000, 10000, 10000, u32::MAX, 10000, 10000, u32::MAX, 10000, 10000]);
	m_ParamRestartLbdBlockingAvrgMult: f64 = single("/restarts/lbd/blocking/average_mult", "Restarts, LBD-average-based blocking strategy: the multiplier in the formula for determining whether the restart is blocked: #assignments > multiplier * assignments-average", 1.4);
	m_ParamRestartLbdBlockingWinSize: u32 = modes("/restarts/lbd/blocking/win_size", "Restarts, LBD-average-based blocking strategy: window size", [5000, 5000, 5000, 5000, 5000, 10000, 5000, 5000, 5000], 1);

	// Parameters: buffer multipliers
	m_ParamMultClss: f64 = single("/multiplier/clauses", "The multiplier for reallocating the clause buffer", MultiplierDef, 1.);
	m_ParamMultVars: f64 = single("/multiplier/variables", "The multiplier for reallocating data structures, indexed by variables (and literals)", 1.33, 1.);
	m_ParamMultWatches: f64 = single("/multiplier/watches", "The multiplier for reallocating the watches buffer", 1.33, 1.);
	m_ParamMultWasteWatches: f64 = single("/multiplier/watches_waste", "The multiplier for triggering reallocation of the watches buffer, when there is too much waste", 1.33, 1.);

	// Parameters: clause simplification
	m_ParamSimplify: bool = single("/deletion/simplify", "Simplify?", true);
	m_ParamSimplifyGlobalLevelScoreStrat: u8 = single("/deletion/simplify_global_level_strat", "Simplify: how to update the score of the only remaining globally assigned variable (relevant only when /backtracking/custom_bt_strat != 0) -- 0: don't touch; 1: minimal out of all globals; 2: maximal out of all globals", 0, 0, 2);

	// Parameters: data-based compression
	m_ParamWastedFractionThrToDelete: f64 = single("/deletion/wasted_fraction_thr", "If wasted > size * this_parameter, we physically compress the memory (0.2 in Fiver, Maple)", 0.2, f64::EPSILON, 1.);
	m_ParamCompressAllocatedPerWatch: bool = single("/deletion/compress_allocated_per_watch", "Tightly compress the allocated memory per every watch during memory compression (or, otherwise, never compress per watch)", true);

	// Parameters: clause deletion
	m_ParamClsDelStrategy: u8 = single("/deletion/clause/strategy", "Clause deletion strategy (0: no clause deletion; 1: Topor; 2: Fiver)", 1, 0, 2);
	m_ParamClsDelDeleteOnlyAssumpDecLevel: bool = single("/deletion/clause/delete_only_assump_dec_level", "Clause deletion: apply clause deletion only when at assumption decision level", false);
	m_ParamClsLowDelActivityDecay: f64 = single("/deletion/clause/activity_decay", "Clause deletion: activity decay factor", 0.999, f64::EPSILON, 1.);

	m_ParamClsDelLowFracToDelete: f32 = modes("/deletion/clause/frac_to_delete", "Clause deletion: the fraction of clauses to delete", [0.7f32, 0.8f32, 0.8f32, 0.8f32, 0.8f32, 0.7f32, 0.8f32, 0.7f32, 0.8f32], f32::EPSILON, 1.0f32);
	m_ParamClsDelLowTriggerInit: u32 = single("/deletion/clause/trigger_init", "Clause deletion: the initial number of learnts to trigger clause deletion", 6000);
	m_ParamClsDelLowTriggerInc: u32 = single("/deletion/clause/trigger_linc", "Clause deletion: the increment in learnts to trigger clause deletion", 75);
	m_ParamClsDelS1LowTriggerMult: f64 = single("/deletion/clause/s1/trigger_inc_mult", "Clause deletion: the change in increment in learnts to trigger clause deletion (that is, the value by which /deletion/clause/trigger_linc is multiplied)", 1.2, 1.);
	m_ParamClsDelS1LowTriggerMax: u32 = single("/deletion/clause/s1/trigger_max", "Clause deletion: the maximal number of learnts to activate clause deletion", 60000);
	m_ParamClsDelS2LowGlue: u8 = modes("/deletion/clause/s2/low_glue", "Clause deletion: the low glue for an additional current-change push", [3, 3, 3, 3, 3, 6, 3, 3, 3]);
	m_ParamClsDelS2MediumGlue: u8 = modes("/deletion/clause/s2/medium_glue", "Clause deletion: the medium glue for an additional current-change push", [5, 5, 5, 5, 5, 8, 5, 5, 5]);
	m_ParamClsDelS2LowMediumIncValue: u32 = single("/deletion/clause/s2/low_medium_inc", "Clause deletion: the increment for the low&medium push to current-change", 1000);

	m_ParamClsDelLowMinGlueFreeze: TUV = modes("/deletion/clause/glue_min_freeze", "Initial query: clause deletion: protect clauses for one round if their glue decreased and is lower than this value", [4, 30, 30, 30, 30, 5, 30, 30, 30], 0, u8::MAX as TUV);
	m_ParamClsDelLowMinGlueFreezeAi: TUV = single("/deletion/clause/glue_min_freeze_ai", "Normal incremental query: clause deletion: protect clauses for one round if their glue decreased and is lower than this value", 30, 0, u8::MAX as TUV);
	m_ParamClsDelLowMinGlueFreezeS: TUV = modes("/deletion/clause/glue_min_freeze_s", "Short incremental query: clause deletion: protect clauses for one round if their glue decreased and is lower than this value", [5, 30, 30, 30, 30, 5, 30, 30, 30], 0, u8::MAX as TUV);
	m_ParamClsDelGlueNeverDelete: TUV = modes("/deletion/clause/glue_never_delete", "Clause deletion: the highest glue value blocking clause deletion", [2, 2, 2, 2, 2, 3, 2, 2, 2], 0, u8::MAX as TUV);
	m_ParamClsDelGlueClusters: TUV = modes("/deletion/clause/glue_clusters", "Clause deletion: the number of glue clusters", [11, 0, 0, 0, 0, 11, 0, 8, 0], 0, u8::MAX as TUV);
	m_ParamClsDelGlueMaxCluster: TUV = modes("/deletion/clause/glue_max_lex_cluster", "Clause deletion: the highest glue which makes the clause belong to a glue-cluster", [16, 0, 0, 0, 0, 16, 0, 16, 3], 0, u8::MAX as TUV);

	// Parameters: phase saving
	m_ParamPhaseMngForceSolution: bool = modes("/phase/force_solution", "Phase management: always force (the polarities of) the latest solution?", [false, false, false, false, false, false, true, false, false]);
	m_ParamPhaseMngRestartsBlockSize: u16 = single("/phase/restarts_block_size", "Phase management: the number of restarts in a block for phase management considerations", 10, 1);
	m_ParamPhaseMngUnforceRestartsFractionInit: f64 = single("/phase/unforce_restarts_fraction_init", "Phase management for the initial query: don't force the polarities for the provided fraction of restarts", 0.25, 0., 1.);
	m_ParamPhaseMngUnforceRestartsFractionN: f64 = single("/phase/unforce_restarts_fraction_n", "Phase management for the normal (non-short) incremental query: don't force the polarities for the provided fraction of restarts", 0., 0., 1.);
	m_ParamPhaseMngUnforceRestartsFractionS: f64 = single("/phase/unforce_restarts_fraction_s", "Phase management for the short incremental query: don't force the polarities for the provided fraction of restarts", 0., 0., 1.);
	m_ParamPhaseMngStartInvStrat: u8 = single("/phase/start_inv_strat", "Phase management: startegy to start an invocation with; relevant when 0 < /phase/unforce_restarts_fraction < 1: 0: start with force; 1: start with unforce; 2: start with rand", 0, 0, 2);
	m_ParamPhaseBoostFlippedForced: bool = single("/phase/boost_flipped_forced", "Phase management: boost the scores of forced variables, flipped by BCP?", false);
	m_ParamUpdateParamsWhenVarFixed: u8 = modes("/phase/update_params_when_var_fixed", "Phase management: update the parameters to pre-defined values, when fixed-only-once (1) or fixed-forever (2) or either (3)", [0, 0, 0, 0, 0, 0, 0, 0, 3], 0, 3);

	// Parameters: inprocessing
	m_ParamInprocessingOn: bool = single("/inprocessing/on", "Inprocessing: on or off", false);
	m_ParamIngInvokeEveryQueryAfterInitPostpone: bool = single("/inprocessing/invoke_every_query_after_init_postpone", "Inprocessing: invoke right after every query after /inprocessing/postpone_first_inv_conflicts conflicts", true);
	m_ParamIngPostponeFirstInvConflicts: u32 = single("/inprocessing/postpone_first_inv_conflicts", "Inprocessing: conflicts to postpone the very first inprocessing invocation", 0);
	m_ParamIngConflictsBeforeNextInvocation: u32 = single("/inprocessing/conflicts_before_next", "Inprocessing: conflicts before the next invocation", u32::MAX);
}

// The state of the C++ CTopiParams class proper (which, unlike here, doesn't hold the parameter values, since they are members of the solver class)
// Used to simulate the copy and the move-assignment of the C++ CTopiParams (see Solve)
#[derive(Clone)]
pub struct TTopiParamsNonValueState
{
	m_ErrorCode: TErrorType,
	m_Mode: TModeType,
	m_ErrorDescr: String,
	m_Name2PrevVal: HashMap<String, String>,
}

impl CTopiParams
{
	// CTopiParams(m_Params): the copy of everything, but the parameter values
	pub fn CopyNonValueState(&self) -> TTopiParamsNonValueState
	{
		TTopiParamsNonValueState { m_ErrorCode: self.m_ErrorCode, m_Mode: self.m_Mode, m_ErrorDescr: self.m_ErrorDescr.clone(), m_Name2PrevVal: self.m_Name2PrevVal.clone() }
	}

	// m_Params = move(copy): restores everything, but the parameter values
	pub fn MoveNonValueState(&mut self, s: TTopiParamsNonValueState)
	{
		self.m_ErrorCode = s.m_ErrorCode;
		self.m_Mode = s.m_Mode;
		self.m_ErrorDescr = s.m_ErrorDescr;
		self.m_Name2PrevVal = s.m_Name2PrevVal;
	}

	pub fn SetParam(&mut self, param_name: &str, param_val: f64)
	{
		if self.IsError()
		{
			return;
		}

		match self.m_Name2DescrUpdateGetval.get(param_name).map(|e| e.1)
		{
			None =>
			{
				let descr = "Parameter ".to_string() + param_name + " doesn't exists. See all parameters description below:\n" + &self.GetAllParamsDescr();
				self.SetError(TErrorType::NAME_DOESNT_EXIST_WHEN_SETTING_PARAMETER, &descr);
			}
			Some(update) =>
			{
				let err_descr = update(self, TValOrMode::Val(param_val));
				if !err_descr.is_empty()
				{
					// The description is only needed for the error message
					let descr = self.m_Name2DescrUpdateGetval[param_name].0.clone();
					self.SetError(TErrorType::ERROR_WHEN_SETTING_PARAMETER, &(Self::GetNameDescr(param_name, &descr) + ": couldn't set this parameter : " + &err_descr));
				}
				else if param_name == m_ModeParamName
				{
					self.m_Mode = param_val as TModeType;
				}
			}
		}

		// The C++ version indexes the per-mode arrays by the provided mode even if it is out of range (which is undefined behavior); we don't change the modes then
		if param_name == m_ModeParamName && (param_val as TModeType) < m_Modes
		{
			let updates: Vec<TUpdateFunc> = self.m_Name2DescrUpdateGetval.iter().filter(|nd| nd.0 != m_ModeParamName).map(|nd| nd.1 .1).collect();
			for update in updates
			{
				update(self, TValOrMode::Mode(param_val as TModeType));
			}
		}
	}

	pub fn IsError(&self) -> bool
	{
		self.m_ErrorCode != TErrorType::NO_ERR
	}

	pub fn GetErrorDescr(&self) -> String
	{
		self.m_ErrorDescr.clone()
	}

	pub fn GetMode(&self) -> TModeType
	{
		self.m_Mode
	}

	pub fn GetAllParamsCurrValues(&mut self) -> String
	{
		let mut header_printed = false;
		let first_invocation = self.m_Name2PrevVal.is_empty();
		let mut ss = String::new();

		let mut prev_class = String::new();
		let names_get_vals: Vec<(String, TGetValFunc)> = self.m_Name2DescrUpdateGetval.iter().map(|p| (p.0.clone(), p.1 .2)).collect();
		for (curr_name, get_val) in names_get_vals
		{
			let curr_val = get_val(self);

			if !first_invocation
			{
				let curr_param_name_2_prev = self.m_Name2PrevVal.get_mut(&curr_name).unwrap();
				if curr_val == *curr_param_name_2_prev
				{
					continue;
				}
				*curr_param_name_2_prev = curr_val.clone();
			}
			else
			{
				self.m_Name2PrevVal.insert(curr_name.clone(), curr_val.clone());
			}

			if !header_printed
			{
				ss += &print_as_color("c Parameter values:", ansi_color_code::red);
				if !first_invocation
				{
					ss += &print_as_color(" (only the modified ones are printed)", ansi_color_code::red);
				}
				ss += "\n";
				header_printed = true;
			}

			let curr_class = Self::GetParamClass(&curr_name);
			if prev_class != curr_class
			{
				ss += Self::SeparatorLine();
				ss += "\n";
			}

			ss += &("c ".to_string() + &print_as_color(&curr_name, ansi_color_code::magenta) + " " + &curr_val + "\n");

			prev_class = curr_class;
		}

		ss += Self::SeparatorLine();
		ss += "\n";

		ss
	}

	pub fn GetAllParamsDescr(&self) -> String
	{
		let mut ss = String::new();

		ss += &print_as_color("c solver library parameters:", ansi_color_code::red);
		ss += "\n";

		let mut prev_class = String::new();
		for curr_param in &self.m_Name2DescrUpdateGetval
		{
			let curr_class = Self::GetParamClass(curr_param.0);
			if prev_class != curr_class
			{
				ss += Self::SeparatorLine();
				ss += "\n";
			}

			ss += &("c ".to_string() + &print_as_color(curr_param.0, ansi_color_code::magenta) + " : " + &curr_param.1 .0 + "\n");

			prev_class = curr_class;
		}

		ss += Self::SeparatorLine();
		ss += "\n";

		ss
	}

	#[inline(always)]
	fn SeparatorLine() -> &'static str
	{
		"************************************************************"
	}

	pub(crate) fn GetParamClass(param_name: &str) -> String
	{
		let first_slash_it = param_name.find('/');
		let second_slash_it = param_name.get(1..).and_then(|s| s.find('/')).map(|i| i + 1);
		match (first_slash_it, second_slash_it)
		{
			(Some(first_slash_it), Some(second_slash_it)) => param_name[first_slash_it..second_slash_it].to_string(),
			_ => String::new(),
		}
	}

	// The part of the parameter's constructor, which touches the parameters' registry: records the error (if any) or registers the parameter
	#[allow(unused_mut)]
	fn OnParamDefined(&mut self, param_name: &str, mut err_info: Option<(TErrorType, String)>, mut updated_descr: Option<String>, update: TUpdateFunc, get_val: TGetValFunc)
	{
		// Test-only fault injection: lets the tests reach the error path of an invalid parameter definition (all the actual definitions are valid)
		#[cfg(test)]
		if param_def_fault::IsFaulty(param_name)
		{
			err_info = Some((TErrorType::ERROR_WHEN_SETTING_PARAMETER, ERR_VAL_BELOW_MIN.to_string()));
			updated_descr = None;
		}
		if let Some((c, d)) = err_info
		{
			self.SetError(c, &d);
		}
		if let Some(updated_descr) = updated_descr
		{
			self.NewParam(param_name, updated_descr, update, get_val);
		}
	}

	pub(crate) fn NewParam(&mut self, param_name: &str, param_descr: String, update: TUpdateFunc, get_val: TGetValFunc)
	{
		if self.IsError()
		{
			return;
		}

		if let Some(param) = self.m_Name2DescrUpdateGetval.get(param_name)
		{
			let descr = "Parameter already exists; internal error; create Topor developers. The parameter: ".to_string() + &Self::GetNameDescr(param_name, &param.0);
			self.SetError(TErrorType::NAME_EXISTS_WHEN_CREATING_PARAMETER, &descr);
		}
		else
		{
			self.m_Name2DescrUpdateGetval.insert(param_name.to_string(), (param_descr, update, get_val));
		}
	}

	fn GetNameDescr(name: &str, descr: &str) -> String
	{
		name.to_string() + " : " + descr
	}

	fn SetError(&mut self, err_code: TErrorType, err_descr: &str)
	{
		self.m_ErrorCode = err_code;
		self.m_ErrorDescr = err_descr.to_string();
	}
}
