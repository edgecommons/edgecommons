# (Re)create the edgecommons credentials Phase 3 real-AWS validation resources.
# Run teardown.ps1 first if they already exist. After running, validate from the lab
# (lab-5950x) by fetching TES creds via the device cert and running a central sync — see
# README.md in this folder for the exact lab command.
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$region = "us-east-1"
$role = "GreengrassV2TokenExchangeRole"
$secretName = "lab-5950x/edgecommons-cred-validation/db/password"

Write-Output "1/2 Creating Secrets Manager secret ..."
aws secretsmanager create-secret --name $secretName --secret-string "validation-secret-v1" `
  --description "edgecommons credentials Phase 3 real-AWS validation (delete after test)" `
  --tags "Key=edgecommons-validation,Value=true" --region $region | Out-String

Write-Output "2/2 Attaching scoped inline policy to $role ..."
aws iam put-role-policy --role-name $role --policy-name edgecommons-cred-validation `
  --policy-document "file://$here/tes-policy.json" --region $region
Write-Output "Setup complete."
