"use strict";
const AdminUserService = require("../services/admin-user-service");
const validation = require("../validations/adminUserValidation");
const commonValidation = require("../validations/commonValidation");

const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");

const AdminUserController = () => {};

const service = new AdminUserService();

// Verify Email
AdminUserController.VerifyEmail = async (req, res, next) => {
  try {
    const { error } = validation.verifyEmailSchema.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }
    const data = await service.verifyEmail(req.body.email);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// Verify Email OTP
AdminUserController.VerifyEmailOTP = async (req, res, next) => {
  try {
    const { error, value } = validation.verifyEmailOTPSchema.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }
    const data = await service.verifyEmailOTP(value.email, value.otp);
    return response(
      res,
      data.status ? RESPONSE_STATUS.OK.code : RESPONSE_STATUS.BAD_REQUEST.code,
      data.status,
      data,
      null
    );
  } catch (error) {
    next(error);
  }
};

// Create Account
AdminUserController.createAccount = async (req, res, next) => {
  try {
    const { error, value } = validation.createAccount.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }
    console.log(value);
    const data = await service.createAccount(value);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// get list of users
AdminUserController.getAdminUser = async (req, res, next) => {
  try {
    const data = await service.getAdminUsersList();
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

//Login
AdminUserController.login = async (req, res, next) => {
  try {
    const { error } = validation.login.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }
    const { login, password } = req.body;
    const data = await service.login(login, password);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// Get user profile by id
AdminUserController.getAdminProfile = async (req, res, next) => {
  try {
    const id = req.params.id;
    const idError = commonValidation.validateIdSchema.validate({ id });
    if (idError.error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        idError.error.details[0].message
      );
    }

    const data = await service.getProfileDetailById(id);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// Update user profile by id
AdminUserController.updateAdminProfile = async (req, res, next) => {
  try {
    const id = req.params.id;
    const idError = commonValidation.validateIdSchema.validate({ id });
    if (idError.error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        idError.error.details[0].message
      );
    }
    const { error, value } = validation.updateAccount.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }

    const data = await service.updateAdminProfileById(id, value);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

//Get Refresh Token
// AdminUserController.GetRefreshToken = async (req, res, next) => {
//   try {
//     const data = await service.VerifyRefreshToken(
//       req.body.id,
//       req.body.refreshToken
//     );
//     return response(res, RESPONSE_STATUS.OK.code, true, data, null);
//   } catch (error) {
//     next(error);
//   }
// };

// AdminUserController.ForgotPassword = async (req, res, next) => {
//   try {
//     const { error } = validation.verifyEmailSchema.validate(req.body);
//     if (error) {
//       return response(
//         res,
//         RESPONSE_STATUS.BAD_REQUEST.code,
//         false,
//         null,
//         error.details[0].message
//       );
//     }
//     const { email } = req.body;
//     const data = await service.forgotPassword(email);
//     return response(res, RESPONSE_STATUS.OK.code, true, data, null);
//   } catch (error) {
//     next(error);
//   }
// };
// Change password
AdminUserController.changePassword = async (req, res, next) => {
  try {
    const id = await validateId(req, res);
    const { error } = validation.changePasswordSchema.validate(req.body);
    if (error) {
      return response(
        res,
        RESPONSE_STATUS.BAD_REQUEST.code,
        false,
        null,
        error.details[0].message
      );
    }
    const { password, newPassword } = req.body;
    const data = await service.changePassword(id, password, newPassword);
    return response(res, RESPONSE_STATUS.OK.code, true, data, null);
  } catch (error) {
    next(error);
  }
};

// AdminUserController.logout = async (req, res, next) => {
//   try {
//     const { error } = validation.logoutSchema.validate(req.body);
//     if (error) {
//       return response(
//         res,
//         RESPONSE_STATUS.BAD_REQUEST.code,
//         false,
//         null,
//         error.details[0].message
//       );
//     }
//     const { userId } = req.body;
//     const data = await service.logout(userId);

//     return response(res, RESPONSE_STATUS.OK.code, true, data, null);
//   } catch (error) {
//     next(error);
//   }
// };
// AdminUserController.deleteAccount = async (req, res, next) => {
//   try {
//     const { error } = validation.deleteAccountSchema.validate(req.body);
//     if (error) {
//       return response(
//         res,
//         RESPONSE_STATUS.BAD_REQUEST.code,
//         false,
//         null,
//         error.details[0].message
//       );
//     }
//     const { userId, password } = req.body;
//     const data = await service.deleteAccount(userId, password);

//     return response(res, RESPONSE_STATUS.OK.code, true, data, null);
//   } catch (error) {
//     next(error);
//   }
// };

const validateId = async (req, res) => {
  const id = req.params.id;
  const { error } = commonValidation.validateIdSchema.validate({ id });
  if (error) {
    return response(
      res,
      RESPONSE_STATUS.BAD_REQUEST.code,
      false,
      null,
      error.details[0].message
    );
  }
  return req.params.id;
};

module.exports = AdminUserController;
