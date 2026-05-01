const Joi = require("joi");

const CommonValidation = () => {};

CommonValidation.validateIdSchema = Joi.object({
  id: Joi.string().uuid().message("id must be uuid").required(),
});

module.exports = CommonValidation;
