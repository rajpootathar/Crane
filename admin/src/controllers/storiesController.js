const { response } = require("../utils/response-handler");
const { RESPONSE_STATUS } = require("../config/constants");
const StoryService = require("../services/story-service");


const StoriesController = () => {};
const service = new StoryService();
StoriesController.fetchAllStories = async (req, res,next) => {
    const {page=1,pageSize=10}=req.params;
    try {
        const data = await service.getAllStories(page,pageSize);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        next(error);
    }
}

StoriesController.deleteStory = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.deleteStory(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        console.log(error);
        next(error);
    }
}
StoriesController.getStoryDetails = async (req, res,next) => {
    const {id}= req.params;
    try {
        const data = await service.getStoryDetails(id);
        return response(res, RESPONSE_STATUS.OK.code, true, data, null);
    }
    catch (error) {
        console.log(error);
        next(error);
    }
}

module.exports = StoriesController;
