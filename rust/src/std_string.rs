// Copyright(C) 2021-2023 Intel Corporation
// SPDX - License - Identifier: MIT

// The equivalents of the C++ <string> numeric conversion functions (std::stod, std::stoi, std::stoll, std::stoul, std::stoull), which the C++ sources use to parse user input
// Exactly as in C++: leading white-spaces are skipped, the longest valid prefix is converted and the rest of the string is ignored
// None stands for the exception thrown by the C++ function (std::invalid_argument, if no conversion could be performed, or std::out_of_range)

fn SkipSpaces(s: &[u8]) -> usize
{
	let mut i = 0;
	// isspace in the "C" locale
	while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
	{
		i += 1;
	}
	i
}

fn StartsWithIgnoreCase(s: &[u8], prefix: &[u8]) -> bool
{
	s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

// std::stod for decimal floating-points, infinity and nan (hexadecimal floating-points aren't supported)
pub fn stod(str: &str) -> Option<f64>
{
	let s = str.as_bytes();
	let start = SkipSpaces(s);
	let mut i = start;
	if i < s.len() && (s[i] == b'+' || s[i] == b'-')
	{
		i += 1;
	}

	let isNeg = s.get(start) == Some(&b'-');
	if StartsWithIgnoreCase(&s[i..], b"inf")
	{
		return Some(if isNeg { f64::NEG_INFINITY } else { f64::INFINITY });
	}
	if StartsWithIgnoreCase(&s[i..], b"nan")
	{
		return Some(f64::NAN);
	}

	let mut digits = 0;
	while i < s.len() && s[i].is_ascii_digit()
	{
		i += 1;
		digits += 1;
	}
	if i < s.len() && s[i] == b'.'
	{
		i += 1;
		while i < s.len() && s[i].is_ascii_digit()
		{
			i += 1;
			digits += 1;
		}
	}
	if digits == 0
	{
		// std::invalid_argument
		return None;
	}
	if i < s.len() && (s[i] == b'e' || s[i] == b'E')
	{
		let mut j = i + 1;
		if j < s.len() && (s[j] == b'+' || s[j] == b'-')
		{
			j += 1;
		}
		if j < s.len() && s[j].is_ascii_digit()
		{
			while j < s.len() && s[j].is_ascii_digit()
			{
				j += 1;
			}
			i = j;
		}
	}

	// A lone trailing '.' (e.g., "5.") is fine for both strtod and Rust
	let res: f64 = str[start..i].parse().ok()?;
	// std::out_of_range (strtod reports ERANGE on overflow)
	if res.is_infinite() { None } else { Some(res) }
}

// The common part of the integer conversions: returns the sign and the magnitude (None, if there are no digits or the magnitude doesn't fit into u128)
fn ParseInteger(str: &str) -> Option<(bool, u128)>
{
	let s = str.as_bytes();
	let mut i = SkipSpaces(s);
	let mut isNeg = false;
	if i < s.len() && (s[i] == b'+' || s[i] == b'-')
	{
		isNeg = s[i] == b'-';
		i += 1;
	}
	if i >= s.len() || !s[i].is_ascii_digit()
	{
		return None;
	}
	let mut res: u128 = 0;
	while i < s.len() && s[i].is_ascii_digit()
	{
		res = res.checked_mul(10)?.checked_add((s[i] - b'0') as u128)?;
		i += 1;
	}
	Some((isNeg, res))
}

pub fn stoll(str: &str) -> Option<i64>
{
	let (isNeg, res) = ParseInteger(str)?;
	if isNeg
	{
		if res > i64::MAX as u128 + 1 { None } else { Some((res as i128).wrapping_neg() as i64) }
	}
	else if res > i64::MAX as u128
	{
		None
	}
	else
	{
		Some(res as i64)
	}
}

pub fn stoi(str: &str) -> Option<i32>
{
	i32::try_from(stoll(str)?).ok()
}

// As strtoull: a minus sign is accepted, in which case the value is negated in the unsigned type
pub fn stoull(str: &str) -> Option<u64>
{
	let (isNeg, res) = ParseInteger(str)?;
	if res > u64::MAX as u128
	{
		return None;
	}
	Some(if isNeg { (res as u64).wrapping_neg() } else { res as u64 })
}

// unsigned long is 64-bit on the supported (LP64) platforms
pub fn stoul(str: &str) -> Option<u64>
{
	stoull(str)
}
