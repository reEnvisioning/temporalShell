pub(crate) const SCALE: i64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interpolation {
    Linear,
    Smooth,
    Step,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Curve {
    frames: Vec<(i64, i64)>,
    pub(crate) interpolation: Interpolation,
}

impl Curve {
    pub(crate) fn sample(&self, progress: i64) -> i64 {
        let progress = progress.clamp(0, SCALE);
        let Some(&(first_at, first_value)) = self.frames.first() else {
            return SCALE;
        };
        if progress <= first_at {
            return first_value;
        }
        for pair in self.frames.windows(2) {
            if progress == pair[1].0 {
                return pair[1].1;
            }
            if progress < pair[1].0 {
                let span = pair[1].0 - pair[0].0;
                let mut local = (progress - pair[0].0) as f64 / span as f64;
                local = match self.interpolation {
                    Interpolation::Linear => local,
                    Interpolation::Smooth => local * local * (3.0 - 2.0 * local),
                    Interpolation::Step => 0.0,
                };
                return (pair[0].1 as f64 + (pair[1].1 as f64 - pair[0].1 as f64) * local).round()
                    as i64;
            }
        }
        self.frames.last().map(|frame| frame.1).unwrap_or(SCALE)
    }

    pub(crate) fn continuous(&self) -> bool {
        self.frames.len() > 1 && self.frames.windows(2).any(|pair| pair[0].1 != pair[1].1)
    }
}

pub(crate) fn constant() -> Curve {
    Curve {
        frames: Vec::new(),
        interpolation: Interpolation::Linear,
    }
}

pub(crate) fn interpolation(value: &str) -> Result<Interpolation, String> {
    match value {
        "linear" => Ok(Interpolation::Linear),
        "smooth" => Ok(Interpolation::Smooth),
        "step" => Ok(Interpolation::Step),
        _ => Err("interpolation must be linear, smooth, or step".into()),
    }
}

pub(crate) fn curve(value: &str, interpolation: Option<Interpolation>) -> Result<Curve, String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or("keyframes must be a quoted string array")?;
    let mut values = Vec::new();
    if !inner.trim().is_empty() {
        for item in inner.split(',') {
            let item = item.trim();
            let item = item
                .strip_prefix('"')
                .and_then(|item| item.strip_suffix('"'))
                .ok_or("keyframes must contain quoted TIME:VALUE strings")?;
            values.push(item.to_owned());
        }
    }
    if values.is_empty() {
        return Err("keyframes must not be empty".into());
    }
    let frames = parse_keyframes(&values)?
        .into_iter()
        .map(|frame| {
            let at = frame.at * SCALE as f64;
            let value = frame.value * SCALE as f64;
            if at <= i64::MIN as f64
                || at >= i64::MAX as f64
                || value <= i64::MIN as f64
                || value >= i64::MAX as f64
            {
                return Err("keyframe is outside the representable range".to_owned());
            }
            Ok((at.round() as i64, value.round() as i64))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Curve {
        frames,
        interpolation: interpolation.unwrap_or(Interpolation::Linear),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Keyframe {
    pub(crate) at: f64,
    pub(crate) value: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Program {
    instructions: Vec<Instruction>,
    scratch: std::cell::RefCell<Vec<f64>>,
}

impl PartialEq for Program {
    fn eq(&self, other: &Self) -> bool {
        self.instructions == other.instructions
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Instruction {
    Number(f64),
    Variable(Variable),
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Negate,
    Function(Function),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variable {
    Phase,
    Keyframe,
    From,
    To,
    Value,
    Elapsed,
    Remaining,
    Duration,
    Position,
    Pixel,
    Coverage,
    Pi,
    E,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Function {
    Abs,
    Min,
    Max,
    Clamp,
    Lerp,
    Step,
    Smoothstep,
    Floor,
    Ceil,
    Round,
    Fract,
    Sin,
    Cos,
    Tan,
    Exp,
    Log,
    Pow,
    Sqrt,
}

impl Function {
    fn arity(self) -> usize {
        match self {
            Self::Abs
            | Self::Floor
            | Self::Ceil
            | Self::Round
            | Self::Fract
            | Self::Sin
            | Self::Cos
            | Self::Tan
            | Self::Exp
            | Self::Log
            | Self::Sqrt => 1,
            Self::Min | Self::Max | Self::Step | Self::Pow => 2,
            Self::Clamp | Self::Lerp | Self::Smoothstep => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Context {
    pub(crate) phase: f64,
    pub(crate) keyframe: f64,
    pub(crate) from: f64,
    pub(crate) to: f64,
    pub(crate) value: f64,
    pub(crate) elapsed: f64,
    pub(crate) remaining: f64,
    pub(crate) duration: f64,
    pub(crate) position: f64,
    pub(crate) pixel: f64,
    pub(crate) coverage: f64,
}

impl Program {
    pub(crate) fn compile(text: &str) -> Result<Self, String> {
        let tokens = tokenize(text)?;
        let instructions = compile(tokens)?;
        let stack_size = validate_stack(&instructions)?;
        Ok(Self {
            instructions,
            scratch: std::cell::RefCell::new(Vec::with_capacity(stack_size)),
        })
    }

    pub(crate) fn evaluate_context(&self, context: Context) -> f64 {
        let mut stack = self.scratch.borrow_mut();
        stack.clear();
        for instruction in &self.instructions {
            match *instruction {
                Instruction::Number(value) => stack.push(value),
                Instruction::Variable(variable) => stack.push(variable_value(variable, context)),
                Instruction::Negate => unary(&mut stack, |value| -value),
                Instruction::Add => binary(&mut stack, |left, right| left + right),
                Instruction::Subtract => binary(&mut stack, |left, right| left - right),
                Instruction::Multiply => binary(&mut stack, |left, right| left * right),
                Instruction::Divide => binary(&mut stack, |left, right| left / right),
                Instruction::Remainder => binary(&mut stack, |left, right| left % right),
                Instruction::Function(function) => apply_function(&mut stack, function),
            }
            if stack.last().is_some_and(|value| !value.is_finite()) {
                *stack.last_mut().expect("checked") = 0.0;
            }
        }
        let value = stack.pop().filter(|value| value.is_finite()).unwrap_or(0.0);
        stack.clear();
        value
    }

    pub(crate) fn uses_time(&self) -> bool {
        self.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Variable(
                    Variable::Phase | Variable::Elapsed | Variable::Remaining | Variable::Duration
                )
            )
        })
    }
}

pub(crate) type Expression = Program;

pub(crate) fn expression(text: &str) -> Result<Expression, String> {
    Program::compile(text)
}

fn unary(stack: &mut Vec<f64>, operation: impl FnOnce(f64) -> f64) {
    let value = stack.pop().unwrap_or(0.0);
    stack.push(operation(value));
}

fn binary(stack: &mut Vec<f64>, operation: impl FnOnce(f64, f64) -> f64) {
    let right = stack.pop().unwrap_or(0.0);
    let left = stack.pop().unwrap_or(0.0);
    stack.push(operation(left, right));
}

fn apply_function(stack: &mut Vec<f64>, function: Function) {
    let mut arguments = [0.0; 3];
    for index in (0..function.arity()).rev() {
        arguments[index] = stack.pop().unwrap_or(0.0);
    }
    let value = match function {
        Function::Abs => arguments[0].abs(),
        Function::Min => arguments[0].min(arguments[1]),
        Function::Max => arguments[0].max(arguments[1]),
        Function::Clamp => {
            if arguments[1] <= arguments[2] {
                arguments[0].clamp(arguments[1], arguments[2])
            } else {
                0.0
            }
        }
        Function::Lerp => arguments[0] + (arguments[1] - arguments[0]) * arguments[2],
        Function::Step => f64::from(arguments[1] >= arguments[0]),
        Function::Smoothstep => {
            if arguments[0] == arguments[1] {
                f64::from(arguments[2] >= arguments[0])
            } else {
                let value =
                    ((arguments[2] - arguments[0]) / (arguments[1] - arguments[0])).clamp(0.0, 1.0);
                value * value * (3.0 - 2.0 * value)
            }
        }
        Function::Floor => arguments[0].floor(),
        Function::Ceil => arguments[0].ceil(),
        Function::Round => arguments[0].round(),
        Function::Fract => arguments[0].fract(),
        Function::Sin => arguments[0].sin(),
        Function::Cos => arguments[0].cos(),
        Function::Tan => arguments[0].tan(),
        Function::Exp => arguments[0].exp(),
        Function::Log => arguments[0].ln(),
        Function::Pow => arguments[0].powf(arguments[1]),
        Function::Sqrt => arguments[0].sqrt(),
    };
    stack.push(if value.is_finite() { value } else { 0.0 });
}

fn variable_value(variable: Variable, context: Context) -> f64 {
    match variable {
        Variable::Phase => context.phase,
        Variable::Keyframe => context.keyframe,
        Variable::From => context.from,
        Variable::To => context.to,
        Variable::Value => context.value,
        Variable::Elapsed => context.elapsed,
        Variable::Remaining => context.remaining,
        Variable::Duration => context.duration,
        Variable::Position => context.position,
        Variable::Pixel => context.pixel,
        Variable::Coverage => context.coverage,
        Variable::Pi => std::f64::consts::PI,
        Variable::E => std::f64::consts::E,
    }
}

pub(crate) fn parse_keyframes(values: &[String]) -> Result<Vec<Keyframe>, String> {
    let mut frames = Vec::with_capacity(values.len());
    for value in values {
        let (at, output) = value.split_once(':').ok_or("keyframe must be TIME:VALUE")?;
        let frame = Keyframe {
            at: percentage(at, "keyframe time")?,
            value: percentage(output, "keyframe value")?,
        };
        if frames
            .last()
            .is_some_and(|previous: &Keyframe| previous.at >= frame.at)
        {
            return Err("keyframe times must increase".into());
        }
        frames.push(frame);
    }
    if frames.len() > 1
        && (frames.first().is_none_or(|frame| frame.at != 0.0)
            || frames.last().is_none_or(|frame| frame.at != 1.0))
    {
        return Err("keyframes must begin at 0% and end at 100%".into());
    }
    Ok(frames)
}

fn percentage(value: &str, name: &str) -> Result<f64, String> {
    let number = value
        .trim()
        .strip_suffix('%')
        .unwrap_or(value.trim())
        .parse::<f64>()
        .map_err(|_| format!("invalid {name}"))?;
    if number.is_finite() {
        Ok(number / 100.0)
    } else {
        Err(format!("{name} must be finite"))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Operator(char),
    Left,
    Right,
    Comma,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index].is_ascii_digit() || bytes[index] == b'.' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_digit()
                    || matches!(bytes[index], b'.' | b'e' | b'E')
                    || matches!(bytes[index], b'+' | b'-')
                        && matches!(bytes[index - 1], b'e' | b'E'))
            {
                index += 1;
            }
            let value = text[start..index]
                .parse::<f64>()
                .map_err(|_| "invalid expression number")?;
            if !value.is_finite() {
                return Err("expression numbers must be finite".into());
            }
            tokens.push(Token::Number(value));
        } else if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token::Name(text[start..index].to_owned()));
        } else {
            tokens.push(match bytes[index] {
                b'+' | b'-' | b'*' | b'/' | b'%' => Token::Operator(bytes[index] as char),
                b'(' => Token::Left,
                b')' => Token::Right,
                b',' => Token::Comma,
                _ => return Err("invalid expression character".into()),
            });
            index += 1;
        }
    }
    if tokens.is_empty() {
        Err("expression must not be empty".into())
    } else {
        Ok(tokens)
    }
}

#[derive(Clone, Debug)]
enum Pending {
    Operator(char, bool),
    Function(Function),
    Left,
}

fn compile(tokens: Vec<Token>) -> Result<Vec<Instruction>, String> {
    let mut output = Vec::new();
    let mut pending = Vec::new();
    let mut expect_value = true;
    let mut index = 0;
    while index < tokens.len() {
        match &tokens[index] {
            Token::Number(value) => {
                output.push(Instruction::Number(*value));
                expect_value = false;
            }
            Token::Name(name) if tokens.get(index + 1) == Some(&Token::Left) => {
                pending.push(Pending::Function(parse_function(name)?));
                expect_value = true;
            }
            Token::Name(name) => {
                output.push(Instruction::Variable(parse_variable(name)?));
                expect_value = false;
            }
            Token::Left => {
                pending.push(Pending::Left);
                expect_value = true;
            }
            Token::Comma => {
                while !matches!(pending.last(), Some(Pending::Left)) {
                    pop_pending(&mut pending, &mut output)?;
                }
                expect_value = true;
            }
            Token::Right => {
                while !matches!(pending.last(), Some(Pending::Left)) {
                    pop_pending(&mut pending, &mut output)?;
                }
                pending.pop().ok_or("unmatched expression parenthesis")?;
                if matches!(pending.last(), Some(Pending::Function(_))) {
                    pop_pending(&mut pending, &mut output)?;
                }
                expect_value = false;
            }
            Token::Operator(operator) => {
                let unary = expect_value && *operator == '-';
                if expect_value && !unary {
                    return Err("expression operator requires a left value".into());
                }
                let precedence = operator_precedence(*operator, unary);
                while matches!(pending.last(), Some(Pending::Operator(old, old_unary)) if operator_precedence(*old, *old_unary) >= precedence)
                {
                    pop_pending(&mut pending, &mut output)?;
                }
                pending.push(Pending::Operator(*operator, unary));
                expect_value = true;
            }
        }
        index += 1;
    }
    if expect_value {
        return Err("expression ends before a value".into());
    }
    while !pending.is_empty() {
        if matches!(pending.last(), Some(Pending::Left)) {
            return Err("unmatched expression parenthesis".into());
        }
        pop_pending(&mut pending, &mut output)?;
    }
    Ok(output)
}

fn pop_pending(pending: &mut Vec<Pending>, output: &mut Vec<Instruction>) -> Result<(), String> {
    output.push(match pending.pop().ok_or("invalid expression")? {
        Pending::Operator(_, true) => Instruction::Negate,
        Pending::Operator('+', false) => Instruction::Add,
        Pending::Operator('-', false) => Instruction::Subtract,
        Pending::Operator('*', false) => Instruction::Multiply,
        Pending::Operator('/', false) => Instruction::Divide,
        Pending::Operator('%', false) => Instruction::Remainder,
        Pending::Operator(_, false) => return Err("invalid expression operator".into()),
        Pending::Function(function) => Instruction::Function(function),
        Pending::Left => return Err("unmatched expression parenthesis".into()),
    });
    Ok(())
}

fn operator_precedence(operator: char, unary: bool) -> u8 {
    if unary {
        3
    } else if matches!(operator, '*' | '/' | '%') {
        2
    } else {
        1
    }
}

fn validate_stack(instructions: &[Instruction]) -> Result<usize, String> {
    let mut depth = 0_usize;
    let mut maximum = 0_usize;
    for instruction in instructions {
        let consumed = match instruction {
            Instruction::Number(_) | Instruction::Variable(_) => {
                depth += 1;
                maximum = maximum.max(depth);
                continue;
            }
            Instruction::Negate => 1,
            Instruction::Add
            | Instruction::Subtract
            | Instruction::Multiply
            | Instruction::Divide
            | Instruction::Remainder => 2,
            Instruction::Function(function) => function.arity(),
        };
        if depth < consumed {
            return Err("expression has missing arguments".into());
        }
        depth = depth - consumed + 1;
    }
    if depth == 1 {
        Ok(maximum)
    } else {
        Err("expression has extra values".into())
    }
}

fn parse_variable(name: &str) -> Result<Variable, String> {
    Ok(match name {
        "phase" => Variable::Phase,
        "keyframe" => Variable::Keyframe,
        "from" => Variable::From,
        "to" => Variable::To,
        "value" => Variable::Value,
        "elapsed" => Variable::Elapsed,
        "remaining" => Variable::Remaining,
        "duration" => Variable::Duration,
        "position" => Variable::Position,
        "pixel" => Variable::Pixel,
        "coverage" => Variable::Coverage,
        "pi" => Variable::Pi,
        "e" => Variable::E,
        _ => return Err(format!("unknown expression variable {name}")),
    })
}

fn parse_function(name: &str) -> Result<Function, String> {
    Ok(match name {
        "abs" => Function::Abs,
        "min" => Function::Min,
        "max" => Function::Max,
        "clamp" => Function::Clamp,
        "lerp" => Function::Lerp,
        "step" => Function::Step,
        "smoothstep" => Function::Smoothstep,
        "floor" => Function::Floor,
        "ceil" => Function::Ceil,
        "round" => Function::Round,
        "fract" => Function::Fract,
        "sin" => Function::Sin,
        "cos" => Function::Cos,
        "tan" => Function::Tan,
        "exp" => Function::Exp,
        "log" => Function::Log,
        "pow" => Function::Pow,
        "sqrt" => Function::Sqrt,
        _ => return Err(format!("unknown expression function {name}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_are_smooth_and_allow_overshoot() {
        let linear = curve(
            "[\"0%:0%\", \"50%:115%\", \"100%:100%\"]",
            Some(Interpolation::Linear),
        )
        .unwrap();
        let smooth = Curve {
            interpolation: Interpolation::Smooth,
            ..linear.clone()
        };
        assert_eq!(linear.sample(0), 0);
        assert!(smooth.sample(SCALE / 2) > SCALE);
        assert_eq!(linear.sample(SCALE), SCALE);
    }

    #[test]
    fn expressions_combine_keyframes_and_physics_safely() {
        let program = Program::compile("keyframe + 0.1 * sin(phase * pi)").unwrap();
        let value = program.evaluate_context(Context {
            phase: 0.5,
            keyframe: 0.5,
            ..Context::default()
        });
        assert!((value - 0.6).abs() < 1e-9);
        assert_eq!(
            Program::compile("1 / 0")
                .unwrap()
                .evaluate_context(Context::default()),
            0.0
        );
        assert_eq!(
            Program::compile("clamp(0, 1, 0)")
                .unwrap()
                .evaluate_context(Context::default()),
            0.0
        );
        assert!(Program::compile("missing + 1").is_err());
        assert!(Program::compile("depth").is_err());
        assert!(curve("[\"0:0\", \"100:1e30\"]", None).is_err());
        assert!(curve("[\"0:0\", \"100:92233720368547760%\"]", None).is_err());
    }

    #[test]
    fn dynamic_values_begin_at_the_observation_and_retarget_from_displayed_value() {
        let program = Program::compile("lerp(from, to, keyframe)").unwrap();
        let evaluate = |from, to, keyframe| {
            program.evaluate_context(Context {
                from,
                to,
                keyframe,
                ..Context::default()
            })
        };
        assert_eq!(evaluate(40.0, 40.0, 1.0), 40.0);
        assert_eq!(evaluate(40.0, 45.0, 0.0), 40.0);
        assert_eq!(evaluate(40.0, 45.0, 0.5), 42.5);
        assert_eq!(evaluate(40.0, 45.0, 1.0), 45.0);
        assert_eq!(evaluate(42.5, 35.0, 0.0), 42.5);
    }
}
