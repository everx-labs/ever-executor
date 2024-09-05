# Release Notes

All notable changes to this project will be documented in this file.

## Version 1.18.17

- Added gosh feature

## Version 1.18.0

- Use modern crates anyhow and thiserror instead of failure

## Version 1.17.7

- Mesh and common message structures support

## Version 1.17.6

- No due payments on credit phase and add payed dues to storage fee in TVM

## Version 1.17.0

- the crate was renamed from `ton_executor` to `ever_executor`
- supported renaming of other crates

## Version 1.16.122

- Do not delete frozen accounts (if related capability set)

## Version 1.16.108

- Deny non-zero cell level in code/data/lib

## Version 1.16.106

- Remove compiler warning
- Remove unwraps that lead to panic

## Version 1.16.85

- Deny ChangeLibrary action when CapSetLibCode is unset

## Version 1.16.40

- Disable debug symbols by default

## Version 1.16.0

- Skiped compute phase for suspended addresses

## Version 1.15.196

- Removed extra crates bas64
- Minor refactoring

## Version 1.15.191

- Supported ever-types version 2.0

## Version 1.15.190

- Add test for CapFeeInGasUnits

## Version: 1.15.188

### News

- check capability for calculating forward and storage fees

## Version: 1.15.183

### Fixes

- check gas limit and credit for overflow

## Version: 1.15.177

### New

- capability CapBounceAfterFailedAction: if transaction fails on Action phase,
bounced message will be produced 

## Version: 1.15.128

### New

- add common submodule

### Fixes

- minor refactor for clippy

## Version: 1.15.121

### Fixes

- support other libs changes
## Version: 1.15.75

### New

- backward compatibility to prev nodes in bounced fee calculating

## Version: 1.5.73

### New

- support behavior modifier mechanism for TVM