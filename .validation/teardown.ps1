# Teardown for the edgecommons credentials Phase 3 real-AWS validation.
# Deletes every resource created by setup (see manifest.json). Idempotent: ignores "not found".
$ErrorActionPreference = "Continue"
$region = "us-east-1"
$role = "GreengrassV2TokenExchangeRole"
$secretArn = "arn:aws:secretsmanager:us-east-1:162499689067:secret:lab-5950x/edgecommons-cred-validation/db/password-xsLLop"

Write-Output "1/2 Removing inline policy 'edgecommons-cred-validation' from $role ..."
aws iam delete-role-policy --role-name $role --policy-name edgecommons-cred-validation 2>&1 | Out-String

Write-Output "2/2 Deleting Secrets Manager secret (force, no recovery) ..."
aws secretsmanager delete-secret --secret-id $secretArn --force-delete-without-recovery --region $region 2>&1 | Out-String

Write-Output "Verify (both should error / be empty):"
aws iam get-role-policy --role-name $role --policy-name edgecommons-cred-validation --region $region 2>&1 | Out-String
Write-Output "Teardown complete."
