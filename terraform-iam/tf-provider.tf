provider "aws" {
  allowed_account_ids = [var.aws_account_id]
  region              = var.region
}
