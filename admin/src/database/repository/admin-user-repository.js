
const { AdminUser } = require("../../../../shared/src/models");
const { RESPONSE_STATUS } = require("../../config/constants");
class AdminUserRepository {
  async createAccount(
    fullName,
    role,
    email,
    profilePicture,
    generatedPassword,
    salt
  ) {
    try {
      const accountDetails = new AdminUser({
        password: generatedPassword,
        fullName,
        role,
        email,
        profilePicture,
        salt,
      });
      return await accountDetails.save();
    } catch (err) {
      return err;
    }
  }

  async findById(id) {
    try {
      const result = await AdminUser.findByPk(id);
      return result;
    } catch (error) {
      return error;
    }
  }

  async getAdminUsersList() {
    try {
      const { count, rows } = await AdminUser.findAndCountAll({
        attributes: ["role", "email", "fullName", "profilePicture", "status"],
      });
      return { count, rows };
    } catch (err) {
      return err;
    }
  }

  async login(value) {
    try {
      const username = await AdminUser.findOne({
        where: {
          email: value,
        },
      });
      if (username) {
        return username;
      } else {
        const error = new Error("Invalid");
        error.status = RESPONSE_STATUS.BAD_REQUEST.code;
        throw error;
      }
    } catch (error) {
      throw error;
    }
  }

  async getProfileDetailById(id) {
    try {
      const userDetails = await AdminUser.findOne({
        where: {
          id: id,
        },
        attributes: ["role", "email", "fullName", "profilePicture", "status"],
      });
      if (userDetails) {
        return userDetails;
      } else {
        const error = new Error("Invalid");
        error.status = RESPONSE_STATUS.BAD_REQUEST.code;
        throw error;
      }
    } catch (error) {
      throw error;
    }
  }

  async updateAdminProfileById(id, values) {
    try {
      const accountDetails = await AdminUser.update(values, {
        where: {
          id: id,
        },
      });
      return accountDetails[0] === 1
        ? "Updated Successfully"
        : "Failed To Update";
    } catch (error) {
      return error;
    }
  }

  async CheckEmailIsAvailable(email) {
    const isExist = await AdminUser.findOne({
      where: {
        email: email,
      },
      attributes: ["email"],
    });
    return isExist ? true : false;
  }

  async GetAccountDetailsById(id, params) {
    try {
      const accountDetails = await AdminUser.findOne({
        where: {
          id: id,
        },
        attributes: params
          ? params
          : [
              "id",
              "fullName",
              "profilePicture",
              "email",
              "identificationNumber",
              "status",
              "createdAt",
              "updatedAt",
            ],
      });
      return accountDetails;
    } catch (error) {
      return error;
    }
  }

  async changePassword(email, newPassword) {
    try {
      const user = await AdminUser.findOne({
        where: {
          email: email,
        },
      });

      if (!user) {
        return {
          email: email,
          status: false,
          message: "User not found",
        };
      }

      const result = await user.update({
        password: newPassword,
      });

      return { result };
    } catch (error) {
      return error;
    }
  }
}

module.exports = AdminUserRepository;
