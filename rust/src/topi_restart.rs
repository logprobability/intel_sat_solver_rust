// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

use crate::topi::*;

extern "C"
{
	fn rand() -> std::ffi::c_int;
}

impl<Ind: TUInd, const Compress: bool> CTopi<Ind, Compress>
{
	pub(crate) fn Restart(&mut self) -> bool
	{
		let mut restartNow = false;

		match self.m_CurrRestartStrat
		{
			RESTART_STRAT_NUMERIC =>
			{
				restartNow = if self.m_DecLevel > 0 && self.m_Params.m_ParamRestartNumericLocal.get()
				{
					self.m_ConfsSinceRestart - self.m_RstNumericLocalConfsSinceRestartAtDecLevelCreation[self.m_DecLevel as usize] >= self.m_RstNumericCurrConfThr
				}
				else
				{
					self.m_ConfsSinceRestart >= self.m_RstNumericCurrConfThr
				};
				if restartNow
				{
					if self.m_Params.m_ParamRestartNumericSubStrat.get() == 1
					{
						// Luby
						self.m_RstNumericCurrConfThr = (Self::RestartLubySequence(self.m_Params.m_ParamRestartLubyConfIncr.get(), self.m_Stat.m_Restarts) * self.m_Params.m_ParamRestartNumericInitConfThr.get() as f64) as u64;
					}
					else
					{
						// Arithmetic
						self.m_RstNumericCurrConfThr += self.m_Params.m_ParamRestartArithmeticConfIncr.get() as u64;
					}
				}
			}
			RESTART_STRAT_LBD =>
			{
				restartNow = self.m_RstGlueLbdWin.IsFullWindow() && self.m_RstGlueLbdWin.GetAverage() * self.m_Params.m_ParamRestartLbdAvrgMult.get() > self.m_RstGlueGlobalLbdSum / self.m_RstGlueAssertingGluedClss as f64;
				if restartNow
				{
					self.m_RstGlueLbdWin.Clear(false);
				}
			}
			_ => {}
		}

		if restartNow
		{
			self.m_ConfsSinceRestart = 0;
			self.m_Stat.m_Restarts += 1;
			self.m_RestartsSinceInvStart += 1;
			self.UpdateAllUipInfoAfterRestart();
			debug_assert!(self.m_QueryCurr != TQueryType::QUERY_NONE);
			let currUnforceRestartsFraction = self.GetCurrUnforceRestartsFraction();
			if currUnforceRestartsFraction != 0. && currUnforceRestartsFraction != 1.
			{
				let restartInBlock: u64 = self.m_RestartsSinceInvStart % self.m_Params.m_ParamPhaseMngRestartsBlockSize.get() as u64;
				let currRestartFractionCompleted: f64 = restartInBlock as f64 / self.m_Params.m_ParamPhaseMngRestartsBlockSize.get() as f64;
				debug_assert!(currRestartFractionCompleted >= 0.);
				let currRestartFractionLeft: f64 = 1. - currRestartFractionCompleted;
				debug_assert!(currRestartFractionLeft >= 0.);
				if self.m_PhaseStage == self.m_PhaseInitStage
				{
					if self.m_PhaseStage == TPhaseStage::PHASE_STAGE_STANDARD && currRestartFractionLeft <= currUnforceRestartsFraction
					{
						self.m_PhaseStage = TPhaseStage::PHASE_STAGE_DONT_FORCE;
						debug_assert!(self.m_PhaseInitStage != self.m_PhaseStage);
						debug_assert!(self.NV(1) || self.P(&("Phase stage changed: ".to_string() + &self.GetPhaseStageStr() + "\n")));
					}
					else if self.m_PhaseStage == TPhaseStage::PHASE_STAGE_DONT_FORCE && currRestartFractionCompleted >= currUnforceRestartsFraction
					{
						self.m_PhaseStage = TPhaseStage::PHASE_STAGE_STANDARD;
						debug_assert!(self.m_PhaseInitStage != self.m_PhaseStage);
						debug_assert!(self.NV(1) || self.P(&("Phase stage changed: ".to_string() + &self.GetPhaseStageStr() + "\n")));
					}
				}
			}
			debug_assert!(self.NV(1) || self.P(&("Restart #".to_string() + &self.m_Stat.m_Restarts.to_string() + "\n")));
		}

		restartNow
	}

	pub(crate) fn RstNewAssertingGluedCls(&mut self, glue: TUV)
	{
		debug_assert!(self.m_CurrRestartStrat == RESTART_STRAT_LBD);

		if self.m_Params.m_ParamRestartLbdBlockingEnable.get()
		{
			self.m_RstGlueBlckAsgnWin.Enqueue(self.m_AssignedVarsNum);
			if self.m_RstGlueAssertingGluedClss > self.m_Params.m_ParamRestartLbdBlockingConfsToConsider.get() as u64 && self.m_RstGlueLbdWin.IsFullWindow() && self.m_AssignedVarsNum as f64 > self.m_Params.m_ParamRestartLbdBlockingAvrgMult.get() * self.m_RstGlueBlckAsgnWin.GetAverage()
			{
				self.m_RstGlueLbdWin.Clear(false);
				self.m_Stat.m_RestartsBlocked += 1;
			}
		}

		let glueThr = if glue > self.m_Params.m_ParamRestartLbdThresholdGlueVal.get() { self.m_Params.m_ParamRestartLbdThresholdGlueVal.get() } else { glue };
		self.m_RstGlueLbdWin.Enqueue(glueThr);
		self.m_RstGlueGlobalLbdSum += glueThr as f64;
	}

	pub(crate) fn RestartInit(&mut self)
	{
		debug_assert!(self.m_QueryCurr != TQueryType::QUERY_NONE);

		fn InitGlue<Ind: TUInd, const Compress: bool>(this: &mut CTopi<Ind, Compress>)
		{
			if this.m_Params.m_ParamRestartLbdBlockingEnable.get()
			{
				this.m_RstGlueBlckAsgnWin.Init(this.m_Params.m_ParamRestartLbdBlockingWinSize.get() as u16);
				if this.m_RstGlueBlckAsgnWin.IsError()
				{
					this.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Couldn't reserve memory for m_RstGlueBlckAsgnWin".to_string());
				}
			}
			this.m_RstGlueLbdWin.Init(this.m_Params.m_ParamRestartLbdWinSize.get() as u16);
			if this.m_RstGlueLbdWin.IsError()
			{
				this.SetStatus(TToporStatus::STATUS_ALLOC_FAILED, "Couldn't reserve memory for m_RstGlueLbdWin".to_string());
			}
		}

		if self.m_QueryCurr == TQueryType::QUERY_INIT
		{
			self.m_CurrRestartStrat = self.m_Params.m_ParamRestartStrategyInit.get();
			if self.m_CurrRestartStrat == RESTART_STRAT_NUMERIC
			{
				self.m_RstNumericCurrConfThr = self.m_Params.m_ParamRestartNumericInitConfThr.get() as u64;
				debug_assert!(self.NV(1) || self.P("Restarts initialized to arithmetic\n"));
			}
			else if self.m_CurrRestartStrat == RESTART_STRAT_LBD
			{
				InitGlue(self);
				debug_assert!(self.NV(1) || self.P("Restarts initialized to glue\n"));
			}
		}
		else if self.m_Params.m_ParamRestartStrategyInit.get() != self.m_Params.m_ParamRestartStrategyS.get() || self.m_Params.m_ParamRestartStrategyInit.get() != self.m_Params.m_ParamRestartStrategyN.get()
		{
			let prevRestartStrat: u8 = self.m_CurrRestartStrat;

			debug_assert!(self.m_QueryCurr == TQueryType::QUERY_INC_SHORT || self.m_QueryCurr == TQueryType::QUERY_INC_NORMAL);
			self.m_CurrRestartStrat = if self.m_QueryCurr == TQueryType::QUERY_INC_SHORT { self.m_Params.m_ParamRestartStrategyS.get() } else { self.m_Params.m_ParamRestartStrategyN.get() };

			if self.m_CurrRestartStrat != prevRestartStrat
			{
				debug_assert!(prevRestartStrat == RESTART_STRAT_LBD || prevRestartStrat == RESTART_STRAT_NUMERIC);
				debug_assert!(self.m_CurrRestartStrat == RESTART_STRAT_LBD || self.m_CurrRestartStrat == RESTART_STRAT_NUMERIC);

				if self.m_CurrRestartStrat == RESTART_STRAT_NUMERIC
				{
					// LBD --> NUMERIC
					debug_assert!(prevRestartStrat == RESTART_STRAT_LBD);
					self.m_RstNumericCurrConfThr = self.m_Stat.m_Conflicts + self.m_Params.m_ParamRestartNumericInitConfThr.get() as u64;
					if self.m_Params.m_ParamRestartLbdBlockingEnable.get()
					{
						self.m_RstGlueBlckAsgnWin.Clear(false);
					}
					self.m_RstGlueLbdWin.Clear(false);
					debug_assert!(self.NV(1) || self.P("Restarts switched to arithmetic\n"));
				}
				else
				{
					// NUMERIC --> LBD
					debug_assert!(prevRestartStrat == RESTART_STRAT_NUMERIC);
					InitGlue(self);
					debug_assert!(self.NV(1) || self.P("Restarts switched to glue\n"));
				}
			}
		}

		let currUnforceRestartsFraction = self.GetCurrUnforceRestartsFraction();

		self.m_PhaseStage =
			if currUnforceRestartsFraction == 0. { TPhaseStage::PHASE_STAGE_STANDARD }
			else if currUnforceRestartsFraction == 1. { TPhaseStage::PHASE_STAGE_DONT_FORCE }
			else if self.m_Params.m_ParamPhaseMngStartInvStrat.get() == 0 { TPhaseStage::PHASE_STAGE_STANDARD }
			else if self.m_Params.m_ParamPhaseMngStartInvStrat.get() == 1 { TPhaseStage::PHASE_STAGE_DONT_FORCE }
			else if unsafe { rand() } as f64 / std::ffi::c_int::MAX as f64 <= currUnforceRestartsFraction { TPhaseStage::PHASE_STAGE_DONT_FORCE }
			else { TPhaseStage::PHASE_STAGE_STANDARD };
		self.m_PhaseInitStage = self.m_PhaseStage;

		self.m_RestartsSinceInvStart = 0;

		debug_assert!(self.NV(1) || self.P(&("Phase stage set: ".to_string() + &self.GetPhaseStageStr() + "\n")));
	}

	pub(crate) fn RestartLubySequence(y: f64, mut x: u64) -> f64
	{
		// Finite subsequences of the Luby-sequence:
		// 0: 1
		// 1: 1 1 2
		// 2: 1 1 2 1 1 2 4
		// 3: 1 1 2 1 1 2 4 1 1 2 1 1 2 4 8
		// ...

		// Find the finite subsequence that contains index 'x' and the size of that subsequence:
		let mut size: u64 = 1;
		let mut seq: u64 = 0;
		while size < x + 1
		{
			seq += 1;
			size = 2 * size + 1;
		}

		while size - 1 != x
		{
			size = (size - 1) >> 1;
			seq -= 1;
			x %= size;
		}

		y.powf(seq as f64)
	}
}
