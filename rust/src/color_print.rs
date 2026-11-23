// Copyright(C) 2021 Intel Corporation
// SPDX - License - Identifier: MIT

use std::fmt::Display;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
#[allow(non_camel_case_types)]
pub enum ansi_color_code
{
	black = 30,
	red = 31,
	green = 32,
	yellow = 33,
	blue = 34,
	magenta = 35,
	cyan = 36,
	white = 37,
	bright_black = 90,
	bright_red = 91,
	bright_green = 92,
	bright_yellow = 93,
	bright_blue = 94,
	bright_magenta = 95,
	bright_cyan = 96,
	bright_white = 97,
	none = 98,
}

pub fn print_as_color<P: Display>(value: P, color: ansi_color_code) -> String
{
	if cfg!(not(windows)) && color != ansi_color_code::none
	{
		format!("\x1b[1;{}m{}\x1b[0m", color as i32, value)
	}
	else
	{
		format!("{}", value)
	}
}
