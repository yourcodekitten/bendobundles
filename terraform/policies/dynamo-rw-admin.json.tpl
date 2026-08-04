{
  "Statement": [
    {
      "Action": [
        "dynamodb:BatchGetItem",
        "dynamodb:ConditionCheckItem",
        "dynamodb:DeleteItem",
        "dynamodb:GetItem",
        "dynamodb:PutItem",
        "dynamodb:Query",
        "dynamodb:Scan",
        "dynamodb:UpdateItem"
      ],
      "Effect": "Allow",
      "Resource": [
        "${table_arn}",
        "${table_arn}/index/*"
      ],
      "Sid": "DataPlane"
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
            "OIDCSTATE#*"
          ]
        }
      },
      "Effect": "Deny",
      "Resource": [
        "${table_arn}"
      ],
      "Sid": "DenyOidcItems"
    }
  ],
  "Version": "2012-10-17"
}
