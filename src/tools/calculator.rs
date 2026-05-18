//! Calculator tool — performs basic arithmetic.
//!
//! This is a toy tool used in Phase 2 integration tests.
//! It supports add, subtract, multiply, and divide operations
//! on two numeric operands.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

/// A calculator tool that performs basic arithmetic.
///
/// Supports: add, subtract, multiply, divide.
/// Returns the result and the operation performed.
pub struct CalculatorTool;

impl Tool for CalculatorTool {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Perform basic arithmetic operations: add, subtract, multiply, divide. Takes two numeric operands and an operation."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"],
                    "description": "The arithmetic operation to perform."
                },
                "a": {
                    "type": "number",
                    "description": "The first operand."
                },
                "b": {
                    "type": "number",
                    "description": "The second operand."
                }
            },
            "required": ["operation", "a", "b"]
        })
    }

    fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
        let operation = args
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'operation' field".into()))?;

        let a = args
            .get("a")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'a' field".into()))?;

        let b = args
            .get("b")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| AgentError::InvalidToolArgs("missing or invalid 'b' field".into()))?;

        let result = match operation {
            "add" => a + b,
            "subtract" => a - b,
            "multiply" => a * b,
            "divide" => {
                if b == 0.0 {
                    return Ok(ToolOutput {
                        success: false,
                        data: json!({ "error": "division by zero" }),
                        summary: "Error: division by zero".into(),
                    });
                }
                a / b
            }
            other => {
                return Err(AgentError::InvalidToolArgs(format!(
                    "unknown operation: {other}"
                )));
            }
        };

        Ok(ToolOutput {
            success: true,
            data: json!({
                "operation": operation,
                "a": a,
                "b": b,
                "result": result,
            }),
            summary: format!("{a} {operation} {b} = {result}"),
        })
    }
}
