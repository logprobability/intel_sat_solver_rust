// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;
use std::io::Write;

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn SVar(&self, v: TUVar) -> String
	{
		let mut ss = String::new();

		let sv = v.to_string();

		ss += &sv;

		if self.IsAssignedVar(v)
		{
			ss += "@";
			ss += &self.m_VarInfo[v as usize].m_DecLevel.to_string();
			let _s = self.m_VsidsHeap.get_var_score(v);
			/*if (s != 0)
			{
				ss << "=" << s;
			}*/
		}

		ss
	}

	pub(crate) fn SLit(&self, l: TULit) -> String
	{
		let mut s = if IsNeg(l) { "-".to_string() + &self.SVar(GetVar(l)) } else { self.SVar(GetVar(l)) };
		if self.IsAssigned(l)
		{
			s += &("[".to_string() + (if self.IsSatisfied(l) { "S" } else { "U" }) + "]");
		}
		s
	}

	pub(crate) fn SE2I(&self) -> String
	{
		let mut ss = String::new();

		ss += "E2I:\n";

		for externalLit in 1..self.m_E2ILitMap.cap() as TLit
		{
			ss += &("\t".to_string() + &externalLit.to_string() + " : " + &self.SLit(self.m_E2ILitMap[externalLit as usize]) + "; ");
		}

		ss += "\n";

		ss
	}

	pub(crate) fn STrail(&self) -> String
	{
		let mut ss = String::new();

		ss += "Current trail (reversed):\n";

		let mut v = self.m_TrailEnd;
		while v != BadUVar
		{
			let l = self.GetAssignedLitForVar(v);
			ss += "\t";

			let vDecLevel = self.GetAssignedDecLevelVar(v);
			let prevDecLevel = if self.m_VarInfo[v as usize].m_TrailPrev == BadUVar { TUV::MAX } else { self.GetAssignedDecLevelVar(self.m_VarInfo[v as usize].m_TrailPrev) };

			if vDecLevel != prevDecLevel
			{
				ss += &(" DL ".to_string() + &vDecLevel.to_string() + " *** ");
			}

			ss += &(self.SLit(l) + " {");

			let ai = self.m_AssignmentInfo[GetVar(l) as usize];
			let vi = self.m_VarInfo[GetVar(l) as usize];

			if ai.m_IsAssigned() && ai.IsAssignedBinary()
			{
				ss += &self.SLit(vi.m_BinOtherLit());
			}
			else if vi.m_ParentClsInd != Self::BadClsInd
			{
				ss += &self.SLits(self.Cls(vi.m_ParentClsInd), false);
			}

			ss += "}; ";

			v = self.m_VarInfo[v as usize].m_TrailPrev;
		}

		ss += "\n";

		ss
	}

	pub(crate) fn SUserLits(&self, litSpan: &[TLit]) -> String
	{
		let mut ss = String::new();

		for l in litSpan
		{
			ss += &l.to_string();
			ss += " ";
		}

		// ss.str().substr(0, ss.str().size() - 1)
		ss.pop();
		ss
	}

	pub(crate) fn SVars(&self, varSpan: TSpanTULit) -> String
	{
		let mut ss = String::new();

		for v in varSpan
		{
			ss += &self.SVar(v);
			ss += " ";
		}

		// ss.str().substr(0, ss.str().size() - 1)
		ss.pop();
		ss
	}

	pub(crate) fn P(&self, s: &str) -> bool
	{
		stdout_print!("{}", s);
		let _ = std::io::stdout().flush();
		true
	}

	pub(crate) fn PrintDebugModel(&mut self, trv: TToporReturnVal)
	{
		if trv != TToporReturnVal::RET_SAT
		{
			stdout_println!("m_DebugModel = FAILED, since the return value is not SAT; it's {}", trv);
			return;
		}
		stdout_print!("m_DebugModel = {{false");

		let mut v: TLit = 1;
		while v <= self.m_Stat.m_MaxUserVar
		{
			let externalVal = self.GetValue(v);
			debug_assert!(externalVal == TToporLitVal::VAL_SATISFIED || externalVal == TToporLitVal::VAL_UNSATISFIED);
			stdout_print!(", {}", if externalVal == TToporLitVal::VAL_SATISFIED { "true" } else { "false" });
			if v % 100 == 0
			{
				stdout_print!("\n");
			}
			v += 1;
		}

		stdout_print!("}};\n");
	}

	// Assumed to be invoked when the trail and the clauses are expected to be consistent with debug-model, that is:
	// The assigned trail must be consistent, while there must be no clauses falsified by the debug-model
	pub(crate) fn VerifyDebugModel(&mut self)
	{
		if self.m_DebugModel.is_empty()
		{
			return;
		}
		debug_assert!(self.m_DebugModel.len() as TLit == self.m_Stat.m_MaxUserVar + 1);
		// -1: undefined; 0: false; 1: true
		let mut internalVarVals: CDynArray<u8> = CDynArray::new_init(self.m_Stat.m_MaxInternalVar as usize + 1, u8::MAX);

		// Note that, precisely as in the C++ version, a uint8_t is compared to -1 after the integral promotion to int, so the comparison never holds
		let ExpectedVal = |internalVarVals: &CDynArray<u8>, l: TULit| -> u8
		{
			let v = GetVar(l);
			let expectedValVar: u8 = internalVarVals[v as usize];
			if expectedValVar as i32 == -1 { return u8::MAX; }

			if IsNeg(l)
			{
				return if expectedValVar == 1 { 0 } else { 1 };
			}

			expectedValVar
		};

		let SInternalLitData = |this: &Self, l: TULit| -> String
		{
			let elit = this.GetExternalLit(l);
			"{index ".to_string() + &l.to_string() + " : ilit " + &this.SLit(l) + " : elit " + &elit.to_string() + "; debugModel = " + &((if elit > 0 { this.m_DebugModel[elit as usize] } else { this.m_DebugModel[(-elit) as usize] }) as u8).to_string() + "}"
		};

		//cout << "E2I: " << endl;
		let mut externalV: TLit = 1;
		while externalV <= self.m_Stat.m_MaxUserVar
		{
			let l: TUVar = self.m_E2ILitMap[externalV as usize];
			let v = GetVar(l);
			internalVarVals[v as usize] = if IsNeg(l) { !self.m_DebugModel[externalV as usize] as u8 } else { self.m_DebugModel[externalV as usize] as u8 };
			//cout << "m_DebugModel[" << externalV << "] = " << m_DebugModel[externalV] << " : " << SLit(l) << endl;
			externalV += 1;
		}

		let mut v = self.m_TrailStart;
		while v != BadUVar
		{
			let l = GetLit(v, false);
			let expectedVal: u8 = ExpectedVal(&internalVarVals, l);
			// The C++ version tests expectedVal != -1 here, which always holds, since the uint8_t is promoted to int
			debug_assert!(self.IsSatisfied(l) == (expectedVal != 0) || self.P(&("\t".to_string() + &self.SLit(GetLit(v, false)) + "\n")));
			if self.IsSatisfied(l) != (expectedVal != 0)
			{
				stdout_print!("VerifyDebugModel ERROR in verifying the trail at literal {}. Exiting...\n", SInternalLitData(self, l));
				std::process::exit(-1);
			}
			v = self.m_VarInfo[v as usize].m_TrailNext;
		}

		let mut l: TULit = 1;
		while l < self.GetNextLit()
		{
			let wi = self.m_Watches[l as usize];

			let expectedValL = ExpectedVal(&internalVarVals, l);
			if wi.IsEmpty() || expectedValL != 0
			{
				l += 1;
				continue;
			}

			if wi.m_BinaryWatches != Ind::ZERO
			{
				let binWatches = self.WLBinWatchesSpan(&wi);
				for secondLit in binWatches
				{
					let expectedValSecondLit = ExpectedVal(&internalVarVals, secondLit);
					debug_assert!(expectedValSecondLit != 0 || self.P(&("\t".to_string() + &self.SLit(l) + " " + &self.SLit(secondLit) + "\n")));
					if expectedValSecondLit == 0
					{
						stdout_println!("[l secondLit] = [{}, {}]", SInternalLitData(self, l), SInternalLitData(self, secondLit));
						stdout_print!("VerifyDebugModel ERROR in verifying a binary clause! Exiting...\n");
						std::process::exit(-1);
					}
				}
			}
			l += 1;
		}

		let mut clsInd = self.ClsLoopFirst(false);
		while !self.ClsLoopCompleted()
		{
			if self.ClsChunkDeleted(clsInd)
			{
				clsInd = self.ClsLoopNext();
				continue;
			}

			let cls = self.ConstClsSpan(clsInd, TUV::MAX);

			let mut isOK = false;
			for l in cls
			{
				let expectedValL = ExpectedVal(&internalVarVals, l);
				if expectedValL != 0
				{
					isOK = true;
					break;
				}
			}

			debug_assert!(isOK || self.P(&("\t".to_string() + &self.SLits(cls, false) + "\n")));
			if !isOK
			{
				for l in cls
				{
					stdout_print!("{}, ", SInternalLitData(self, l));
				}
				stdout_print!("\nVerifyDebugModel ERROR in verifying a long clause! Exiting...\n");
				std::process::exit(-1);
			}
			clsInd = self.ClsLoopNext();
		}

		stdout_print!("VerifyDebugModel VERIFIED!\n");
	}
}
