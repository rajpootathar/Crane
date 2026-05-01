const Axios = require('axios');
const {
	FEED_PROXY,
} = require('../config');
class StoryService {
	constructor() {
	}
    async getAllStories(page,pageSize) {
		try {
			const result = await Axios.get(`${FEED_PROXY}/stories?page=${page}&pageSize=${pageSize}`);
			const  {data}  = result.data;
			return  data ;
		} catch (error) {
			throw new Error(error);
		}
	} 
    async deleteStory(id) {
		try {
			const result = await Axios.delete(`${FEED_PROXY}/stories/${id}`);
			const  {data}  = result.data;
			return  data ;
		} catch (error) {
			throw new Error(error);
		}
	} 
    async getStoryDetails(id) {
		try {
			const result = await Axios.get(`${FEED_PROXY}/stories/${id}`);
			const  {data}  = result.data;
			return  data ;
		} catch (error) {
			throw new Error(error);
		}
	}
    async getUserStoryDetails(id) {
		try {
			const result = await Axios.get(`${FEED_PROXY}/stories/${id}`);
			const  {data}  = result.data;
			return  data ;
		} catch (error) {
			throw new Error(error);
		}
	} 
}
module.exports = StoryService;
