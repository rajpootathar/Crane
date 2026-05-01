const { AdminUserRepository } = require("../database");
const { GenerateOTP, renderHTML } = require("../utils");
const constants = require("../config/constants");
const authentication = require("../utils/authentication");
const { SendMail } = require("../utils/email");

const { redisHSet, redisHGet, setOnCache } = require("../utils/redis");

const { RESPONSE_STATUS } = require("../config/constants");

class AdminUserService {
  constructor() {
    this.repository = new AdminUserRepository();
  }

  async verifyEmail(email) {
    try {
      const isEmailExists = await this.repository.CheckEmailIsAvailable(email);
      if (isEmailExists) {
        const error = new Error("email already exist");
        error.status = RESPONSE_STATUS.BAD_REQUEST.code;
        throw error;
      }
      const otp = GenerateOTP();
      const data = {
        otp: otp,
      };

      const result = await SendMail(
        email,
        "Email Verification From OneVibe",
        null,
        renderHTML("emailVerification.ejs", data)
      );
      if (result) {
        await redisHSet(constants.REDIS.YAD_EMAIL_VERIFICATION, email, data);
        return "Email Sent";
      } else {
        throw new Error("Email Not Sent");
      }
    } catch (error) {
      throw error;
    }
  }

  async verifyEmailOTP(email, otp) {
    try {
      const result = await redisHGet(
        constants.REDIS.YAD_EMAIL_VERIFICATION,
        email
      );
      if (result && result.otp && otp) {
        if (result.otp.toString() === otp.toString()) {
          return {
            email: email,
            status: true,
            message: "verified",
          };
        } else {
          const error = new Error("Invalid OTP");
          error.status = RESPONSE_STATUS.BAD_REQUEST.code;
          throw error;
        }
      } else {
        const error = new Error("email not found");
        error.status = RESPONSE_STATUS.BAD_REQUEST.code;
        throw error;
      }
    } catch (error) {
      throw error;
    }
  }

  async createAccount(values) {
    try {
      const { fullName, email, password, profilePicture, role } = values;
      const salt = await authentication.GenerateSalt();
      const generatedPassword = await authentication.GeneratePassword(
        password,
        salt
      );
      // TODO create sequence number
      const { id } = await this.repository.createAccount(
        fullName,
        role,
        email,
        profilePicture,
        generatedPassword,
        salt
      );
      return id;
    } catch (error) {
      throw error;
    }
  }

  async getAdminUsersList() {
    try {
      const data = await this.repository.getAdminUsersList();
      return data;
    } catch (error) {
      throw error;
    }
  }

  async login(login, password) {
    try {
      const user = await this.repository.login(login);
      if (user) {
        const isPasswordValid = await authentication.ValidatePassword(
          password,
          user.password,
          user.salt
        );
        if (isPasswordValid) {
          return this.generateToken(user.id);
        } else {
          const error = new Error("Invalid");
          error.status = RESPONSE_STATUS.BAD_REQUEST.code;
          throw error;
        }
      }
    } catch (error) {
      throw error;
    }
  }

  async getProfileDetailById(id) {
    try {
      const result = await this.repository.getProfileDetailById(id);
      return result;
    } catch (error) {
      throw error;
    }
  }

  async updateAdminProfileById(id, value) {
    try {
      const result = await this.repository.updateAdminProfileById(id, value);
      return result;
    } catch (error) {
      throw error;
    }
  }

  // async VerifyRefreshToken(userId, refreshToken) {
  //   try {
  //     const result = await authentication.VerifyRefreshToken(
  //       userId,
  //       refreshToken
  //     );
  //     if (result) {
  //       return this.generateToken(userId);
  //     } else {
  //       const error = new Error("Invalid refresh token");
  //       error.statusCode = RESPONSE_STATUS.BAD_REQUEST.code;
  //       throw error;
  //     }
  //   } catch (error) {
  //     throw new Error(error);
  //   }
  // }

  generateToken = async (id) => {
    // await removeCache(id);
    const token = authentication.GenerateSignature({ id });
    const refreshToken = authentication.GenerateRefreshToken({ id });
    await setOnCache(id, refreshToken, 2592000);
    return { userId: id, token, refreshToken };
  };

  // async forgotPassword(email) {
  //   try {
  //     const emailExists = await this.repository.CheckEmailIsAvailable(email);
  //     if (!emailExists) {
  //       return { email: email, status: false, message: "Email not found" };
  //     }
  //     const otp = GenerateOTP();
  //     await setOnCache(otp, email, 900);

  //     const subject = "Reset your YandexGram Password";
  //     const text = `Your OTP is ${otp}. It expires in 15 minutes`;
  //     const html = `Your OTP is ${otp}. It expires in 15 minutes`;
  //     const result = SendMail(email, subject, text, html);

  //     if (result) {
  //       return {
  //         email: email,
  //         status: true,
  //         message: "Email to reset your password has been Sent",
  //       };
  //     } else {
  //       const error = new Error("Something went wrong");
  //       error.statusCode = RESPONSE_STATUS.BAD_REQUEST.code;
  //       throw error;
  //     }
  //   } catch (error) {
  //     throw new Error(error);
  //   }
  // }

  // change password
  async changePassword(id, password, newPassword) {
    try {
      const userDetails = await this.repository.findById(id);

      if (userDetails) {
        const valid = await authentication.ValidatePassword(
          password,
          userDetails.dataValues.password,
          userDetails.dataValues.salt
        );
        if (valid) {
          const salt = await authentication.GenerateSalt();
          const generatedPassword = await authentication.GeneratePassword(
            newPassword,
            salt
          );
          const data = { password: generatedPassword, salt };
          const result = await this.repository.updateAdminProfileById(id, data);
          return result;
        } else {
          const error = new Error("invalid password");
          error.status = RESPONSE_STATUS.BAD_REQUEST.code;
          throw error;
        }
      } else {
        const error = new Error("invalid user");
        error.status = RESPONSE_STATUS.BAD_REQUEST.code;
        throw error;
      }
    } catch (error) {
      throw error;
    }
  }

  // async logout(userId) {
  //   try {
  //     const idExists = await this.repository.checkUserIdExists(userId);
  //     if (!idExists) {
  //       return { status: false, message: "Invalid user" };
  //     }
  //     const result = await removeCache(userId);
  //     if (result === undefined) {
  //       return { status: true, message: "User is not logged in." };
  //     } else {
  //       return { status: true, message: "User logged out successfully." };
  //     }
  //   } catch (error) {
  //     throw Error;
  //   }
  // }
  // async deleteAccount(userId, password) {
  //   try {
  //     const userCredentials = await this.repository.getUserCredentials(userId);
  //     if (!userCredentials) {
  //       return {
  //         status: false,
  //         message: "Invalid user",
  //       };
  //     }
  //     if (userCredentials.isDeleted) {
  //       return {
  //         status: false,
  //         message: "Account has already been deleted",
  //       };
  //     }

  //     const passwordMatches = await authentication.ValidatePassword(
  //       password,
  //       userCredentials.password,
  //       userCredentials.salt
  //     );

  //     if (!passwordMatches) {
  //       return {
  //         status: false,
  //         message: "Invalid password",
  //       };
  //     }
  //     await removeCache(userId);
  //     const accountDeleted = await this.repository.softDeleteAccount(userId);
  //     if (!accountDeleted) {
  //       return { userId, status: false, message: "Failed to delete account" };
  //     }
  //     return { userId, status: true, message: "Account deleted successfully" };
  //   } catch (error) {
  //     throw error;
  //   }
  // }
}

module.exports = AdminUserService;
