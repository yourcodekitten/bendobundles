variable "aws_account_id" {
  type        = string
  description = "Account the bendobundles stack lives in (wrong-profile guard)."
}

variable "region" {
  type        = string
  default     = "us-east-1"
  description = "Region the stack + its resources live in."
}

variable "namespace" {
  type        = string
  default     = "brd"
  description = "Org namespace for context/labels (matches the main stack)."
}

variable "role" {
  type        = string
  default     = "production"
  description = "Context role (matches the main stack)."
}

variable "state_bucket" {
  type        = string
  default     = "brd-prod-ue1-tfstate-store"
  description = "Terraform state bucket. The deploy role gets read/write on the MAIN stack's state prefix here — NOT this IAM stack's prefix."
}

variable "main_stack_state_prefix" {
  type        = string
  default     = "bendobundles"
  description = "workspace_key_prefix of the MAIN stack's state (what the deploy role may read/write)."
}
