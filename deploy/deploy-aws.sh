#!/bin/sh
set -eu
: "${RING_RELEASE_BUCKET:?Set the private S3 bucket retaining the native release archive}"
: "${RING_RELEASE_KEY:?Set the immutable S3 key containing ring-server and web/}"
: "${RING_RELEASE_SHA256:?Set the verified 64-character SHA-256 of the native release archive}"
: "${RING_SECRETS_ARN:?Set Secrets Manager JSON environment ARN}"
: "${RING_VPC_ID:?Set reviewed VPC ID}"
: "${RING_SUBNET_ID:?Set public subnet ID}"
case "$RING_RELEASE_SHA256" in
  *[!a-f0-9]*) echo 'RING_RELEASE_SHA256 must be lowercase hexadecimal.' >&2; exit 1 ;;
esac
[ "${#RING_RELEASE_SHA256}" -eq 64 ] || { echo 'RING_RELEASE_SHA256 must contain 64 characters.' >&2; exit 1; }
ring_region=${RING_AWS_REGION:-${AWS_REGION:-${AWS_DEFAULT_REGION:-us-west-1}}}
ring_stack=${RING_STACK_NAME:-silicon-ring}
aws cloudformation deploy --region "$ring_region" --template-file deploy/aws.yaml --stack-name "$ring_stack" \
  --capabilities CAPABILITY_IAM --no-fail-on-empty-changeset \
  --parameter-overrides "ReleaseBucket=$RING_RELEASE_BUCKET" "ReleaseKey=$RING_RELEASE_KEY" \
  "ReleaseSha256=$RING_RELEASE_SHA256" "SecretsArn=$RING_SECRETS_ARN" \
  "VpcId=$RING_VPC_ID" "SubnetId=$RING_SUBNET_ID" "InstanceType=${RING_INSTANCE_TYPE:-t3.small}"
aws cloudformation describe-stacks --region "$ring_region" --stack-name "$ring_stack" --query 'Stacks[0].Outputs' --output table
cat <<'NEXT'
New instances bootstrap the verified native Rust bundle and Caddy through systemd.
Existing instances do not rerun cloud-init when the stack's UserData changes.
After a stack update, explicitly apply the release with the native updater:

  python3 deploy/update-instance.py \
    --instance-id INSTANCE_ID_FROM_STACK_OUTPUT \
    --release-bucket "$RING_RELEASE_BUCKET" \
    --release-key "$RING_RELEASE_KEY" \
    --release-sha256 "$RING_RELEASE_SHA256" \
    --secret-arn "$RING_SECRETS_ARN" \
    --assets-bucket ASSETS_BUCKET_FROM_STACK_OUTPUT \
    --region REGION_USED_ABOVE

The updater checks readiness and restores the previous release on failure.
Publish signed CLI metadata separately at:
  /var/lib/ring-public/releases/release-index.json
Caddy serves it at https://ring.teamofsilicons.com/releases/release-index.json.
NEXT
