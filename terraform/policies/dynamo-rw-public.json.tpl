{
  "Statement": [
    {
      "Action": [
        "dynamodb:BatchGetItem",
        "dynamodb:ConditionCheckItem",
        "dynamodb:DeleteItem",
        "dynamodb:GetItem",
        "dynamodb:PutItem",
        "dynamodb:Query"
      ],
      "Effect": "Allow",
      "Resource": [
        "${table_arn}",
        "${table_arn}/index/*"
      ],
      "Sid": "DataPlaneNoScanNoUpdate"
    },
    {
      "Action": [
        "dynamodb:UpdateItem"
      ],
      "Condition": {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": [
            "body",
            "claim_id",
            "gsi1pk",
            "gsi1sk",
            "pk",
            "sk",
            "status"
          ]
        },
        "ForAllValues:StringLike": {
          "dynamodb:LeadingKeys": [
            "GAME#*"
          ]
        },
        "StringEqualsIfExists": {
          "dynamodb:ReturnValues": [
            "NONE",
            "UPDATED_OLD",
            "UPDATED_NEW"
          ]
        }
      },
      "Effect": "Allow",
      "Resource": [
        "${table_arn}"
      ],
      "Sid": "ScopedUpdateGAME"
    },
    {
      "Action": [
        "dynamodb:UpdateItem"
      ],
      "Condition": {
        "ForAllValues:StringEquals": {
          "dynamodb:Attributes": [
            "body",
            "claims_allowed",
            "claims_used",
            "expires_at",
            "pk",
            "revoked",
            "sk",
            "thank_note",
            "thanked_at"
          ]
        },
        "ForAllValues:StringLike": {
          "dynamodb:LeadingKeys": [
            "LINK#*"
          ]
        },
        "StringEqualsIfExists": {
          "dynamodb:ReturnValues": [
            "NONE",
            "UPDATED_OLD",
            "UPDATED_NEW"
          ]
        }
      },
      "Effect": "Allow",
      "Resource": [
        "${table_arn}"
      ],
      "Sid": "ScopedUpdateLINK"
    },
    {
      "Action": [
        "dynamodb:BatchGetItem",
        "dynamodb:ConditionCheckItem",
        "dynamodb:DeleteItem",
        "dynamodb:GetItem",
        "dynamodb:PutItem",
        "dynamodb:Query",
        "dynamodb:UpdateItem"
      ],
      "Condition": {
        "ForAnyValue:StringLike": {
          "dynamodb:LeadingKeys": [
            "SESSION#*",
            "SYNC#*"
          ]
        }
      },
      "Effect": "Deny",
      "Resource": [
        "${table_arn}"
      ],
      "Sid": "DenySessionAndSyncItems"
    }
  ],
  "Version": "2012-10-17"
}
