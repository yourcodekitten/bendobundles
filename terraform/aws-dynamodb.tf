module "label_table" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "table"
}

resource "aws_dynamodb_table" "this" {
  name         = module.label_table.id
  billing_mode = "PAY_PER_REQUEST"

  # Table-level hash_key/range_key are NOT deprecated in provider 6.x (verified
  # against the provider schema); only the GSI-level ones are, hence the mixed
  # style: args here, key_schema blocks inside the GSIs.
  hash_key  = "pk"
  range_key = "sk"

  attribute {
    name = "pk"
    type = "S"
  }
  attribute {
    name = "sk"
    type = "S"
  }
  attribute {
    name = "gsi1pk"
    type = "S"
  }
  attribute {
    name = "gsi1sk"
    type = "S"
  }
  attribute {
    name = "gsi2pk"
    type = "S"
  }
  attribute {
    name = "gsi2sk"
    type = "S"
  }

  global_secondary_index {
    name            = "listable"
    projection_type = "ALL"

    key_schema {
      attribute_name = "gsi1pk"
      key_type       = "HASH"
    }
    key_schema {
      attribute_name = "gsi1sk"
      key_type       = "RANGE"
    }
  }

  global_secondary_index {
    name            = "pending-claims"
    projection_type = "ALL"

    key_schema {
      attribute_name = "gsi2pk"
      key_type       = "HASH"
    }
    key_schema {
      attribute_name = "gsi2sk"
      key_type       = "RANGE"
    }
  }

  # Sessions (and the STEAMOWN cache) carry a numeric `ttl` epoch that schema.rs
  # writes. DynamoDB TTL deletion is lazy, so the code also checks expiry itself
  # — TTL lag is harmless; this attribute just reclaims storage after the fact.
  ttl {
    attribute_name = "ttl"
    enabled        = true
  }

  point_in_time_recovery {
    enabled = true
  }

  tags = module.label_table.tags
}
