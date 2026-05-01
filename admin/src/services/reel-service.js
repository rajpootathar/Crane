const Axios = require("axios");
const { FEED_PROXY } = require("../config");
class ReelService {
  constructor() {}
  async getAllReels(page, pageSize) {
    try {
      const { data } = await Axios.get(
        `${FEED_PROXY}/reels?page=${page}&pageSize=${pageSize}`
      );
      return data.success ? data.data : [];
    } catch (error) {
      throw new Error(error);
    }
  }
  async deleteReel(id) {
    try {
      const result = await Axios.delete(`${FEED_PROXY}/reels/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getReelDetails(id) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/reels/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
  async getUserReelDetails(id) {
    try {
      const result = await Axios.get(`${FEED_PROXY}/reels/${id}`);
      const { data } = result.data;
      return data;
    } catch (error) {
      throw new Error(error);
    }
  }
}
module.exports = ReelService;
