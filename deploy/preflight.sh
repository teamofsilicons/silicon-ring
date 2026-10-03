#!/bin/sh
set -eu
# Read-only checks. No provider keys are printed.
cargo test --workspace
npm --prefix web ci
npm --prefix web run build
npm --prefix web test
iam login status --json
iam system version --json
aws sts get-caller-identity --query '{Account:Account,Arn:Arn}' --output json
gh auth status
aws cloudformation validate-template --template-body file://deploy/aws.yaml --query Description --output text
