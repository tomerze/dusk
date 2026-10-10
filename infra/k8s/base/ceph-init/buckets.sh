#!/bin/bash
set -euo pipefail

secrets=${DUSK_SECRETS_DIRECTORY:-/run/secrets}
endpoint=${S3_ENDPOINT:-http://ceph:8080}
evidence_bucket=${EVIDENCE_BUCKET:-dusk-ledger-evidence}
evidence_mode=${EVIDENCE_RETENTION_MODE:-COMPLIANCE}
evidence_days=${EVIDENCE_RETENTION_DAYS:-400}
lake_bucket=${LAKE_BUCKET:-dusk-lake}
files_bucket=${FILES_BUCKET:-dusk-files}
signoz_bucket=${SIGNOZ_BUCKET:-signoz-cold}
evidence_writer=${EVIDENCE_WRITER_USER:-ledger-writer}
evidence_reader=${EVIDENCE_READER_USER:-admin}
files_reader=${FILES_READER_USER:-admin}
lake_writer=${LAKE_WRITER_USER:-vector}
lake_reader=${LAKE_READER_USER:-reader}
files_writer=${FILES_WRITER_USER:-dawn}
signoz_user=${SIGNOZ_USER:-signoz}

AWS_ACCESS_KEY_ID=$(cat "${ADMIN_ACCESS_KEY_FILE:-$secrets/ceph-admin/access-key}")
AWS_SECRET_ACCESS_KEY=$(cat "${ADMIN_SECRET_KEY_FILE:-$secrets/ceph-admin/secret-key}")
export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY
export AWS_DEFAULT_REGION=${S3_REGION:-us-east-1}
export AWS_EC2_METADATA_DISABLED=true
s3api=(aws --endpoint-url "$endpoint" --cli-connect-timeout 10 --cli-read-timeout 60 s3api)

attempt=0
until "${s3api[@]}" list-buckets > /dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then
        echo "the object store at $endpoint did not answer after $attempt attempts" >&2
        exit 1
    fi
    delay=$((RANDOM % (1 << (attempt < 5 ? attempt : 5)) + 1))
    echo "waiting ${delay}s for the object store at $endpoint (attempt $attempt)"
    sleep "$delay"
done

principal() {
    printf 'arn:aws:iam:::user/%s' "$1"
}

statement() {
    printf '{"Sid":"%s","Effect":"%s","Principal":{"AWS":["%s"]},"Action":%s,"Resource":%s}' "$1" "${5:-Allow}" "$(principal "$2")" "$3" "$4"
}

write_only='["s3:GetObject","s3:GetObjectVersion","s3:GetObjectAcl","s3:PutObjectAcl","s3:DeleteObject","s3:DeleteObjectVersion","s3:ListBucket","s3:ListBucketVersions","s3:PutObjectRetention","s3:PutObjectLegalHold","s3:BypassGovernanceRetention"]'

bucket_exists() {
    "${s3api[@]}" head-bucket --bucket "$1" > /dev/null 2>&1
}

ensure_bucket() {
    if bucket_exists "$1"; then
        echo "bucket $1 exists"
    else
        "${s3api[@]}" create-bucket --bucket "$1" > /dev/null
        echo "bucket $1 created"
    fi
}

put_policy() {
    "${s3api[@]}" put-bucket-policy --bucket "$1" --policy "{\"Version\":\"2012-10-17\",\"Statement\":[$2]}"
    echo "bucket $1 policy applied"
}

if bucket_exists "$evidence_bucket"; then
    lock=$("${s3api[@]}" get-object-lock-configuration --bucket "$evidence_bucket" --query 'ObjectLockConfiguration.ObjectLockEnabled' --output text 2>/dev/null || true)
    if [ "$lock" != Enabled ]; then
        echo "bucket $evidence_bucket exists without object lock; it cannot hold ledger evidence. Move its objects elsewhere, delete it and run this job again" >&2
        exit 1
    fi
    echo "bucket $evidence_bucket exists with object lock"
else
    "${s3api[@]}" create-bucket --bucket "$evidence_bucket" --object-lock-enabled-for-bucket > /dev/null
    echo "bucket $evidence_bucket created with object lock"
fi
"${s3api[@]}" put-object-lock-configuration --bucket "$evidence_bucket" \
    --object-lock-configuration "{\"ObjectLockEnabled\":\"Enabled\",\"Rule\":{\"DefaultRetention\":{\"Mode\":\"$evidence_mode\",\"Days\":$evidence_days}}}"
echo "bucket $evidence_bucket default retention $evidence_mode for $evidence_days days"
put_policy "$evidence_bucket" "$(statement EvidenceWriter "$evidence_writer" '["s3:PutObject"]' "[\"arn:aws:s3:::$evidence_bucket\",\"arn:aws:s3:::$evidence_bucket/*\"]"),$(statement EvidenceWriterOnly "$evidence_writer" "$write_only" "[\"arn:aws:s3:::$evidence_bucket\",\"arn:aws:s3:::$evidence_bucket/*\"]" Deny),$(statement EvidenceReader "$evidence_reader" '["s3:GetObject","s3:GetObjectVersion","s3:GetObjectRetention","s3:GetObjectLegalHold","s3:ListBucket","s3:ListBucketVersions","s3:GetBucketObjectLockConfiguration"]' "[\"arn:aws:s3:::$evidence_bucket\",\"arn:aws:s3:::$evidence_bucket/*\"]")"

for bucket in "$lake_bucket" "$files_bucket" "$signoz_bucket"; do
    ensure_bucket "$bucket"
done

put_policy "$lake_bucket" "$(statement LakeWriter "$lake_writer" '["s3:PutObject","s3:GetObject","s3:ListBucket","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"]' "[\"arn:aws:s3:::$lake_bucket\",\"arn:aws:s3:::$lake_bucket/*\"]"),$(statement LakeReader "$lake_reader" '["s3:GetObject","s3:ListBucket"]' "[\"arn:aws:s3:::$lake_bucket\",\"arn:aws:s3:::$lake_bucket/*\"]")"
put_policy "$files_bucket" "$(statement FilesWriter "$files_writer" '["s3:PutObject","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"]' "[\"arn:aws:s3:::$files_bucket\",\"arn:aws:s3:::$files_bucket/*\"]"),$(statement FilesWriterOnly "$files_writer" "$write_only" "[\"arn:aws:s3:::$files_bucket\",\"arn:aws:s3:::$files_bucket/*\"]" Deny),$(statement FilesReader "$files_reader" '["s3:GetObject","s3:DeleteObject","s3:ListBucket"]' "[\"arn:aws:s3:::$files_bucket\",\"arn:aws:s3:::$files_bucket/*\"]")"
put_policy "$signoz_bucket" "$(statement SignozCold "$signoz_user" '["s3:PutObject","s3:GetObject","s3:DeleteObject","s3:ListBucket","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"]' "[\"arn:aws:s3:::$signoz_bucket\",\"arn:aws:s3:::$signoz_bucket/*\"]")"
"${s3api[@]}" put-bucket-lifecycle-configuration --bucket "$files_bucket" \
    --lifecycle-configuration '{"Rules":[{"ID":"abort-incomplete-uploads","Status":"Enabled","Filter":{"Prefix":""},"AbortIncompleteMultipartUpload":{"DaysAfterInitiation":1}}]}'
echo "bucket $files_bucket aborts incomplete multipart uploads after one day"
