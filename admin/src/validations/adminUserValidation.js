const Joi = require("joi");

const UserValidation = () => {};

UserValidation.verifyPhoneNumberSchema = Joi.object({
  phoneNumber: Joi.string()
    .pattern(/^\d{6,14}$/)
    .required()
    .messages({
      "string.pattern.base":
        "The phone number should have The phone number should have minimum 6 digits and maximum 14 digits",
    }),
  countryCode: Joi.string()
    .pattern(/^\+\d{1,3}$/)
    .required()
    .messages({
      "string.pattern.base":
        "The country code should have + and minimum 1 digits and maximum 3 digits",
    }),
});

UserValidation.verifyPhoneNumberOTPSchema = Joi.object({
  phoneNumber: Joi.string()
    .pattern(/^\d{6,14}$/)
    .required()
    .messages({
      "string.pattern.base":
        "The phone number should have The phone number should have minimum 6 digits and maximum 14 digits",
    }),
  countryCode: Joi.string()
    .pattern(/^\+\d{1,3}$/)
    .required()
    .messages({
      "string.pattern.base":
        "The country code should have + and minimum 1 digits and maximum 3 digits",
    }),
  otp: Joi.string()
    .pattern(/^\d{6}$/)
    .required()
    .messages({
      "string.pattern.base": "The otp should have 6 digits",
    }),
});

UserValidation.verifyEmailSchema = Joi.object({
  email: Joi.string().email().required(),
});

UserValidation.verifyEmailOTPSchema = Joi.object({
  email: Joi.string().email().required(),
  otp: Joi.string()
    .pattern(/^\d{6}$/)
    .required()
    .messages({
      "string.pattern.base": "The otp should have 6 digits",
    }),
});

UserValidation.createAccount = Joi.object({
  fullName: Joi.string().min(3).max(50).required(),
  role: Joi.string().required(),
  email: Joi.string().email().required(),
  notify: Joi.boolean().required(),
  password: Joi.string()
    .min(8)
    .max(30)
    .pattern(/^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)[a-zA-Z\d@$._!%*?&]+$/)
    .required()
    .messages({
      "string.pattern.base":
        "The password must contain at least one lowercase letter, one uppercase letter, and one digit",
    }),
});

UserValidation.updateAccount = Joi.object({
  fullName: Joi.string().min(3).max(50).required().messages({
	"string.base": "full name is required",
	"string.min": "full name must have at least {#limit} characters",
	"string.max": "full name must have at most {#limit} characters"
  }),
});

UserValidation.login = Joi.object({
  login: Joi.string().min(3).max(50).required(),
  password: Joi.string()
    .min(8)
    .max(30)
    .pattern(/^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)[a-zA-Z\d@$._!%*?&]+$/)
    .required()
    .messages({
      "string.pattern.base":
        "The password must contain at least one lowercase letter, one uppercase letter, and one digit",
    }),
});

UserValidation.changePasswordSchema = Joi.object({
  password: Joi.string()
    .min(8)
    .max(30)
    .pattern(/^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)[a-zA-Z\d@$!%*?&]+$/)
    .required()
    .messages({
      "string.pattern.base":
        "The password must contain at least one lowercase letter, one uppercase letter, and one digit",
    }),
  newPassword: Joi.string()
    .min(8)
    .max(30)
    .pattern(/^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)[a-zA-Z\d@$!%*?&]+$/)
    .required()
    .messages({
      "string.pattern.base":
        "The new password must contain at least one lowercase letter, one uppercase letter, and one digit",
    })
});

UserValidation.logoutSchema = Joi.object({
  userId: Joi.string()
    .guid({ version: "uuidv4" }) // Validate UUID v4
    .required(),
});

UserValidation.deleteAccountSchema = Joi.object({
  userId: Joi.string()
    .guid({ version: "uuidv4" }) // Validate UUID v4
    .required(),
  password: Joi.string()
    .min(8)
    .max(30)
    .pattern(/^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)[a-zA-Z\d@$!%*?&]+$/)
    .required()
    .messages({
      "string.pattern.base":
        "The password must contain at least one lowercase letter, one uppercase letter, and one digit",
    }),
});

module.exports = UserValidation;
