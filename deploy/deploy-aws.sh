#!/bin/sh
set -eu
: "${RING_IMAGE_URI:?Set immutable ECR image URI including @sha256 digest}"
: "${RING_SECRETS_ARN:?Set Secrets Manager JSON environment ARN}"
: "${RING_VPC_ID:?Set reviewed VPC ID}"
: "${RING_SUBNET_ID:?Set public subnet ID}"
aws cloudformation deploy --template-file deploy/aws.yaml --stack-name silicon-ring \
  --capabilities CAPABILITY_IAM \
  --parameter-overrides "ImageUri=$RING_IMAGE_URI" "SecretsArn=$RING_SECRETS_ARN" \
  "VpcId=$RING_VPC_ID" "SubnetId=$RING_SUBNET_ID" "InstanceType=${RING_INSTANCE_TYPE:-t3.small}"
aws cloudformation describe-stacks --stack-name silicon-ring --query 'Stacks[0].Outputs' --output table
