require("dotenv").config();

const accountSid = process.env.TWILIO_ACCOUNT_SID;
const authToken = process.env.TWILIO_AUTH_TOKEN;
const client = require("twilio")(accountSid, authToken);

module.exports = {
  sendSms: async (phone) => {
    try {
      const verification = await client.verify.v2
        .services(process.env.TWILIO_SERVICE_SID)
        .verifications.create({ to: phone, channel: "sms" });
      return verification.status;
    } catch (error) {
      throw new Error(error);
    }
  },
  verifySms: async (phone, code) => {
    try {
      const verification = await client.verify.v2
        .services(process.env.TWILIO_SERVICE_SID)
        .verificationChecks.create({ to: phone, code: code });
      return verification.status;
    } catch (error) {
      throw new Error(error);
    }
  },
};
